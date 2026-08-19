//! Headless EQ / via-PC CPU profiler for live Avtoradio Opus.
//!
//! Usage:
//!   set ROCKCAST_PROFILE=1
//!   set ROCKCAST_RELAY_ADVERTISE_IP=127.0.0.1
//!   cargo run --release --example eq_profile

use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use rockcast::{
    observers::{IcyWatcher, SpectrumAnalyzer},
    profile,
    relay::StreamRelay,
};

const AVTORADIO_OPUS_URL: &str = "http://play.global.audio/avtoradio.opus";
const WARMUP: Duration = Duration::from_secs(3);
const MEASURE: Duration = Duration::from_secs(15);

#[cfg(windows)]
mod cpu {
    use std::mem::MaybeUninit;
    use std::time::Instant;

    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    fn ft_to_ns(ft: FILETIME) -> u128 {
        let v = ((ft.dwHighDateTime as u128) << 32) | ft.dwLowDateTime as u128;
        v * 100
    }

    fn cpu_time_ns() -> u128 {
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
            ft_to_ns(*kernel.as_ptr()) + ft_to_ns(*user.as_ptr())
        }
    }

    pub struct Sampler {
        start_wall: Instant,
        start_cpu: u128,
    }

    impl Sampler {
        pub fn new() -> Self {
            Self {
                start_wall: Instant::now(),
                start_cpu: cpu_time_ns(),
            }
        }

        pub fn cpu_percent(&self) -> f64 {
            let wall = self.start_wall.elapsed().as_nanos() as f64;
            let cpu = (cpu_time_ns().saturating_sub(self.start_cpu)) as f64;
            if wall == 0.0 {
                0.0
            } else {
                100.0 * cpu / wall
            }
        }
    }
}

#[cfg(not(windows))]
mod cpu {
    use std::time::Instant;

    pub struct Sampler {
        start_wall: Instant,
    }

    impl Sampler {
        pub fn new() -> Self {
            Self {
                start_wall: Instant::now(),
            }
        }

        pub fn cpu_percent(&self) -> f64 {
            let _ = self.start_wall.elapsed();
            eprintln!("CPU sampling is only implemented on Windows in this example.");
            0.0
        }
    }
}

struct RelayHarness {
    relay: StreamRelay,
    public_url: String,
    tap_url: String,
    consumer_stop: Arc<AtomicBool>,
    consumer: Option<thread::JoinHandle<()>>,
}

impl RelayHarness {
    fn start() -> Self {
        unsafe {
            std::env::set_var("ROCKCAST_RELAY_ADVERTISE_IP", "127.0.0.1");
        }
        let relay = StreamRelay::new();
        let cancel = AtomicBool::new(false);
        let (public_url, content_type) = relay
            .start(
                AVTORADIO_OPUS_URL,
                "127.0.0.1",
                "audio/ogg; codecs=opus",
                &cancel,
            )
            .expect("relay start");
        assert_eq!(content_type, "audio/wav");
        let tap_url = relay.tap_url().expect("tap url");
        assert!(
            relay.wait_for_data(64 * 1024, Duration::from_secs(20)),
            "relay never produced decoded PCM"
        );
        let consumer_stop = Arc::new(AtomicBool::new(false));
        let consumer = Some(spawn_stream_consumer(
            public_url.clone(),
            Arc::clone(&consumer_stop),
        ));
        Self {
            relay,
            public_url,
            tap_url,
            consumer_stop,
            consumer,
        }
    }
}

impl Drop for RelayHarness {
    fn drop(&mut self) {
        self.consumer_stop.store(true, Ordering::Relaxed);
        self.relay.stop();
        // Detach — consumer may still be blocked on a live relay read.
        if let Some(j) = self.consumer.take() {
            drop(j);
        }
        unsafe {
            std::env::remove_var("ROCKCAST_RELAY_ADVERTISE_IP");
        }
    }
}

fn spawn_stream_consumer(public_url: String, stop: Arc<AtomicBool>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let rest = public_url.strip_prefix("http://").expect("http url");
        let host_port = rest.split('/').next().expect("host:port");
        let Ok(mut stream) = TcpStream::connect(host_port) else {
            return;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let request = format!(
            "GET /stream HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n"
        );
        let Ok(()) = stream.write_all(request.as_bytes()) else {
            return;
        };
        let mut buf = [0u8; 16 * 1024];
        while !stop.load(Ordering::Relaxed) {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn wait(duration: Duration) {
    thread::sleep(duration);
}

fn run_scenario(name: &str, setup: impl FnOnce() -> Box<dyn FnMut() + Send>) {
    eprintln!("\n=== scenario: {name} ===");
    profile::reset();
    let mut tick = setup();
    wait(WARMUP);
    let cpu = cpu::Sampler::new();
    let started = Instant::now();
    while started.elapsed() < MEASURE {
        tick();
        thread::sleep(Duration::from_millis(16));
    }
    let cpu_pct = cpu.cpu_percent();
    eprintln!("process CPU (1 core ~= 100%): {cpu_pct:.2}% over {:.1}s", MEASURE.as_secs_f64());
    profile::report(name);
    eprintln!("=== scenario {name} done ===");
}

fn main() {
    unsafe {
        std::env::set_var("ROCKCAST_PROFILE", "1");
    }
    let _ = env_logger::builder().filter_level(log::LevelFilter::Warn).try_init();

    eprintln!("RockCast EQ profiler — Avtoradio Opus, warmup {WARMUP:?}, measure {MEASURE:?}");

    run_scenario("relay_only", || {
        let harness = RelayHarness::start();
        eprintln!("relay public={}", harness.public_url);
        eprintln!("relay tap={}", harness.tap_url);
        Box::new(move || {
            let _ = &harness;
        })
    });

    run_scenario("via_pc_eq_off_icy", || {
        let harness = RelayHarness::start();
        let (tx, _rx) = mpsc::channel();
        let mut icy = IcyWatcher::new();
        icy.start(harness.tap_url.clone(), tx);
        eprintln!("icy watcher on tap={}", harness.tap_url);
        Box::new(move || {
            let _ = (&harness, &icy);
        })
    });

    run_scenario("via_pc_eq_on", || {
        let harness = RelayHarness::start();
        let mut spectrum = SpectrumAnalyzer::new();
        spectrum.start(harness.tap_url.clone(), None);
        eprintln!("spectrum on tap={}", harness.tap_url);
        Box::new(move || {
            let _ = &harness;
            let levels = spectrum.levels();
            let peak = levels.iter().cloned().fold(0.0f32, f32::max);
            std::hint::black_box(peak);
        })
    });

    run_scenario("direct_eq_on", || {
        let mut spectrum = SpectrumAnalyzer::new();
        spectrum.start(AVTORADIO_OPUS_URL.into(), None);
        eprintln!("spectrum on upstream={AVTORADIO_OPUS_URL}");
        Box::new(move || {
            let levels = spectrum.levels();
            let peak = levels.iter().cloned().fold(0.0f32, f32::max);
            std::hint::black_box(peak);
        })
    });
}
