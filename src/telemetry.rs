//! Periodic playback telemetry written to `rockcast.log` as `METRICS` lines.

use std::time::{Duration, Instant};

use crate::profile;

#[derive(Clone, Copy, Debug)]
pub struct PlaybackSnapshot {
    pub playing: bool,
    pub eq_enabled: bool,
    pub cast_relay: bool,
    pub playing_local: bool,
    pub fast_repaint: bool,
}

pub struct Telemetry {
    interval: Duration,
    last_report: Instant,
    frames: u32,
    cpu: CpuSampler,
}

impl Telemetry {
    pub fn new() -> Self {
        Self {
            interval: Duration::from_secs(5),
            last_report: Instant::now(),
            frames: 0,
            cpu: CpuSampler::new(),
        }
    }

    pub fn on_frame(&mut self) {
        if !profile::enabled() {
            return;
        }
        self.frames += 1;
    }

    pub fn maybe_log(&mut self, snap: PlaybackSnapshot) {
        if !profile::enabled() {
            return;
        }
        if !snap.playing {
            self.last_report = Instant::now();
            self.frames = 0;
            self.cpu.reset();
            return;
        }

        let elapsed = self.last_report.elapsed();
        if elapsed < self.interval {
            return;
        }

        let wall_secs = elapsed.as_secs_f64().max(0.001);
        let ui_fps = self.frames as f64 / wall_secs;
        let cpu_pct = self.cpu.cpu_percent(wall_secs);
        let profile = profile::snapshot_line();

        let profile_suffix = if profile.is_empty() {
            String::new()
        } else {
            format!(" {profile}")
        };

        log::info!(
            "METRICS cpu_pct={cpu_pct:.1} ui_fps={ui_fps:.1} playing=1 eq={} relay={} local={} fast_repaint={}{profile_suffix}",
            u8::from(snap.eq_enabled),
            u8::from(snap.cast_relay),
            u8::from(snap.playing_local),
            u8::from(snap.fast_repaint),
        );

        self.last_report = Instant::now();
        self.frames = 0;
        self.cpu.reset();
        profile::reset();
    }
}

impl Default for Telemetry {
    fn default() -> Self {
        Self::new()
    }
}

struct CpuSampler {
    #[cfg(windows)]
    start_cpu_ns: u128,
}

impl CpuSampler {
    fn new() -> Self {
        Self {
            #[cfg(windows)]
            start_cpu_ns: process_cpu_ns(),
        }
    }

    fn reset(&mut self) {
        #[cfg(windows)]
        {
            self.start_cpu_ns = process_cpu_ns();
        }
    }

    fn cpu_percent(&self, wall_secs: f64) -> f64 {
        #[cfg(windows)]
        {
            let cpu_ns = process_cpu_ns().saturating_sub(self.start_cpu_ns) as f64;
            let wall_ns = wall_secs * 1e9;
            if wall_ns <= 0.0 {
                0.0
            } else {
                100.0 * cpu_ns / wall_ns
            }
        }
        #[cfg(not(windows))]
        {
            let _ = wall_secs;
            0.0
        }
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
        ft_to_ns(*kernel.as_ptr()) + ft_to_ns(*user.as_ptr())
    }
}

#[cfg(windows)]
fn ft_to_ns(ft: windows_sys::Win32::Foundation::FILETIME) -> u128 {
    let v = ((ft.dwHighDateTime as u128) << 32) | ft.dwLowDateTime as u128;
    v * 100
}
