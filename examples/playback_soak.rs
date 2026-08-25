//! Real-device playback soak test for MP3, AAC and Opus.
//!
//! Examples (Windows PowerShell):
//!   cargo run --release --example playback_soak -- --mode local --cycles 10
//!   $env:ROCKCAST_SOAK_CAST_NAME='Living room'; cargo run --release --example playback_soak -- --mode cast-via-pc
//!
//! The runner writes CSV and JSONL artefacts to `target/rockcast-soak/<run-id>`.

use std::{
    env,
    fs::{self, File},
    io::Write,
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rockcast::{
    observers::SpectrumAnalyzer,
    output::{self, OutputDevice},
    playback::{PlaybackController, PlaybackEvent},
    profile,
    stations::Station,
};

const DEFAULT_MP3: &str = "https://stream.rockantenne.de/heavy-metal/stream/mp3";
const DEFAULT_AAC: &str = "https://stream.rockantenne.de/heavy-metal/stream/aacp";
const DEFAULT_OPUS: &str = "http://play.global.audio/avtoradio.opus";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Local,
    CastDirect,
    CastViaPc,
}

impl Mode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "local" => Ok(Self::Local),
            "cast-direct" => Ok(Self::CastDirect),
            "cast-via-pc" => Ok(Self::CastViaPc),
            _ => Err(format!(
                "unknown --mode '{value}' (local, cast-direct, cast-via-pc)"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::CastDirect => "cast-direct",
            Self::CastViaPc => "cast-via-pc",
        }
    }
}

struct Config {
    mode: Mode,
    cycles: usize,
    play_for: Duration,
    volume: u8,
    device_name: Option<String>,
}

fn main() -> Result<(), String> {
    // Must be set before constructing controller workers, because profile uses OnceLock.
    unsafe { env::set_var("ROCKCAST_PROFILE", "1") };
    let config = parse_args()?;
    let output_dir = output_dir()?;
    let mut csv = File::create(output_dir.join("cycles.csv")).map_err(|e| e.to_string())?;
    let mut jsonl = File::create(output_dir.join("events.jsonl")).map_err(|e| e.to_string())?;
    writeln!(
        csv,
        "cycle,codec,eq,mode,result,elapsed_ms,cpu_pct,workers,profile,error"
    )
    .map_err(|e| e.to_string())?;

    let device = select_device(&config)?;
    eprintln!("Soak output: {} ({})", device.name(), config.mode.as_str());
    eprintln!("Artefacts: {}", output_dir.display());

    let mut controller = PlaybackController::new();
    let stations = stations();
    let mut failures = 0usize;
    for cycle in 0..config.cycles {
        let station = stations[cycle % stations.len()].clone();
        let eq = cycle % 2 == 0;
        profile::reset();
        let started = Instant::now();
        let cpu = CpuSampler::new();
        let result = run_cycle(&mut controller, &device, &station, eq, &config);
        let elapsed_ms = started.elapsed().as_millis();
        let cpu_pct = cpu.percent();
        let workers = profile::worker_snapshot_line();
        let profile_line = profile::snapshot_line();
        let (status, error) = match result {
            Ok(()) => ("ok", String::new()),
            Err(error) => {
                failures += 1;
                ("error", error)
            }
        };
        let error_csv = error.replace('"', "'").replace(',', ";");
        writeln!(
            csv,
            "{cycle},{},{},{},{status},{elapsed_ms},{cpu_pct:.2},\"{}\",\"{}\",\"{error_csv}\"",
            station.codec,
            u8::from(eq),
            config.mode.as_str(),
            workers.replace('"', "'"),
            profile_line.replace('"', "'")
        )
        .map_err(|e| e.to_string())?;
        writeln!(
            jsonl,
            "{}",
            serde_json::json!({
                "cycle": cycle,
                "codec": station.codec,
                "station": station.name,
                "eq": eq,
                "mode": config.mode.as_str(),
                "result": status,
                "error": error,
                "elapsed_ms": elapsed_ms,
                "cpu_pct": cpu_pct,
                "workers": workers,
                "profile": profile_line,
            })
        )
        .map_err(|e| e.to_string())?;
        csv.flush().map_err(|e| e.to_string())?;
        jsonl.flush().map_err(|e| e.to_string())?;
        eprintln!(
            "cycle {}/{}: {status} {} eq={} cpu={cpu_pct:.1}%",
            cycle + 1,
            config.cycles,
            station.codec,
            u8::from(eq)
        );
    }
    controller.shutdown();
    if failures == 0 {
        Ok(())
    } else {
        Err(format!(
            "{failures} soak cycles failed; inspect {}",
            output_dir.display()
        ))
    }
}

fn run_cycle(
    controller: &mut PlaybackController,
    device: &OutputDevice,
    station: &Station,
    eq: bool,
    config: &Config,
) -> Result<(), String> {
    let generation = controller.play(
        station.clone(),
        device.clone(),
        config.volume,
        config.mode == Mode::CastViaPc,
        eq,
    );
    wait_for_play(controller, generation, Duration::from_secs(20))?;

    if config.mode == Mode::CastViaPc {
        let url = controller
            .relay_public_url()
            .ok_or("Via PC did not expose relay URL")?;
        if !url.starts_with("http://") || !controller.relay_active() {
            return Err("Via PC relay is not active after PlayOk".into());
        }
        if controller.relay_tap_url().is_none() {
            return Err("Via PC did not expose PCM tap for EQ".into());
        }
    }

    let mut direct_spectrum = if config.mode == Mode::CastDirect && eq {
        let mut spectrum = SpectrumAnalyzer::new();
        spectrum.start(station.url.clone(), None);
        Some(spectrum)
    } else {
        None
    };
    let deadline = Instant::now() + config.play_for;
    let mut spectrum_moved = !eq;
    while Instant::now() < deadline {
        if eq {
            let levels = if let Some(spectrum) = direct_spectrum.as_ref() {
                spectrum.levels()
            } else if config.mode == Mode::CastViaPc {
                controller.relay_levels()
            } else {
                controller.local_levels()
            };
            spectrum_moved |= levels.iter().any(|level| (level - 0.08).abs() > 0.03);
        }
        thread::sleep(Duration::from_millis(200));
    }
    drop(direct_spectrum.take());
    if !spectrum_moved {
        return Err("EQ spectrum never moved".into());
    }

    let stop_generation = controller.stop();
    wait_for_stop(controller, stop_generation, Duration::from_secs(8))?;
    // Detached cancellation must settle before the next case; this makes a worker leak visible.
    thread::sleep(Duration::from_millis(600));
    Ok(())
}

fn wait_for_play(
    controller: &mut PlaybackController,
    generation: u64,
    timeout: Duration,
) -> Result<(), String> {
    wait_for_event(controller, generation, timeout, |event| match event {
        PlaybackEvent::PlayOk { .. } => Some(Ok(())),
        PlaybackEvent::Error { message, .. } => Some(Err(message.clone())),
        _ => None,
    })
}

fn wait_for_stop(
    controller: &mut PlaybackController,
    generation: u64,
    timeout: Duration,
) -> Result<(), String> {
    wait_for_event(controller, generation, timeout, |event| match event {
        PlaybackEvent::StopOk { .. } => Some(Ok(())),
        PlaybackEvent::Error { message, .. } => Some(Err(message.clone())),
        _ => None,
    })
}

fn wait_for_event(
    controller: &mut PlaybackController,
    generation: u64,
    timeout: Duration,
    mut done: impl FnMut(&PlaybackEvent) -> Option<Result<(), String>>,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(event) = controller.try_event()
            && controller.apply_event(&event)
            && event_generation(&event) == generation
            && let Some(result) = done(&event)
        {
            return result;
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "timeout waiting for playback event generation={generation}"
    ))
}

fn event_generation(event: &PlaybackEvent) -> u64 {
    match event {
        PlaybackEvent::Status { generation, .. }
        | PlaybackEvent::Title { generation, .. }
        | PlaybackEvent::PlayOk { generation, .. }
        | PlaybackEvent::StopOk { generation }
        | PlaybackEvent::Error { generation, .. } => *generation,
    }
}

fn select_device(config: &Config) -> Result<OutputDevice, String> {
    let (devices, status) = output::scan_all(Duration::from_secs(8), rockcast::i18n::Lang::Ru);
    eprintln!("Device scan: {status}");
    let cast_name = env::var("ROCKCAST_SOAK_CAST_NAME").ok();
    let desired = match config.mode {
        Mode::Local => config.device_name.as_deref(),
        Mode::CastDirect | Mode::CastViaPc => cast_name.as_deref(),
    };
    devices
        .into_iter()
        .find(|device| {
            let kind_matches = (config.mode == Mode::Local) == device.is_local();
            kind_matches && desired.is_none_or(|name| device.name().eq_ignore_ascii_case(name))
        })
        .ok_or_else(|| match config.mode {
            Mode::Local => "no local output device found".into(),
            _ => "no matching Cast device; set ROCKCAST_SOAK_CAST_NAME".into(),
        })
}

fn stations() -> Vec<Station> {
    vec![
        station(
            "MP3",
            env::var("ROCKCAST_SOAK_MP3_URL").unwrap_or_else(|_| DEFAULT_MP3.into()),
            "mp3",
        ),
        station(
            "AAC",
            env::var("ROCKCAST_SOAK_AAC_URL").unwrap_or_else(|_| DEFAULT_AAC.into()),
            "aacp",
        ),
        station(
            "Opus",
            env::var("ROCKCAST_SOAK_OPUS_URL").unwrap_or_else(|_| DEFAULT_OPUS.into()),
            "opus",
        ),
    ]
}

fn station(name: &str, url: String, codec: &str) -> Station {
    Station::from_primary(
        format!("soak-{}", codec),
        name.into(),
        url,
        "soak".into(),
        String::new(),
        0,
        codec.into(),
    )
}

fn parse_args() -> Result<Config, String> {
    let mut mode = Mode::Local;
    let mut cycles = 30;
    let mut play_secs = 12;
    let mut volume = 5;
    let mut device_name = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("missing value after {arg}"))
        };
        match arg.as_str() {
            "--mode" => mode = Mode::parse(&value()?)?,
            "--cycles" => cycles = value()?.parse().map_err(|_| "--cycles must be a number")?,
            "--play-secs" => play_secs = value()?.parse().map_err(|_| "--play-secs must be a number")?,
            "--volume" => volume = value()?.parse().map_err(|_| "--volume must be a number")?,
            "--device" => device_name = Some(value()?),
            "--help" | "-h" => return Err("usage: playback_soak --mode local|cast-direct|cast-via-pc [--cycles 30] [--play-secs 12] [--volume 5] [--device NAME]".into()),
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    Ok(Config {
        mode,
        cycles,
        play_for: Duration::from_secs(play_secs),
        volume: volume.min(100),
        device_name,
    })
}

fn output_dir() -> Result<PathBuf, String> {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let path = PathBuf::from("target")
        .join("rockcast-soak")
        .join(id.to_string());
    fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(windows)]
struct CpuSampler {
    started: Instant,
    cpu_ns: u128,
}

#[cfg(windows)]
impl CpuSampler {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            cpu_ns: process_cpu_ns(),
        }
    }
    fn percent(&self) -> f64 {
        let wall = self.started.elapsed().as_nanos().max(1) as f64;
        100.0 * process_cpu_ns().saturating_sub(self.cpu_ns) as f64 / wall
    }
}

#[cfg(windows)]
fn process_cpu_ns() -> u128 {
    use std::mem::MaybeUninit;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    unsafe {
        let mut creation = MaybeUninit::uninit();
        let mut exit = MaybeUninit::uninit();
        let mut kernel = MaybeUninit::uninit();
        let mut user = MaybeUninit::uninit();
        GetProcessTimes(
            GetCurrentProcess(),
            creation.as_mut_ptr(),
            exit.as_mut_ptr(),
            kernel.as_mut_ptr(),
            user.as_mut_ptr(),
        );
        let ft = |v: windows_sys::Win32::Foundation::FILETIME| {
            ((v.dwHighDateTime as u128) << 32 | v.dwLowDateTime as u128) * 100
        };
        ft(*kernel.as_ptr()) + ft(*user.as_ptr())
    }
}

#[cfg(not(windows))]
struct CpuSampler;

#[cfg(not(windows))]
impl CpuSampler {
    fn new() -> Self {
        Self
    }
    fn percent(&self) -> f64 {
        0.0
    }
}
