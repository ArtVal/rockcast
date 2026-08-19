//! Spectrum analyzer for direct station URLs (non-relay playback).

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use parking_lot::Mutex;

pub use crate::audio::spectrum::BANDS;
use crate::audio::spectrum::BANDS as SPECTRUM_BANDS;

/// Streaming FFT levels consumed by the UI at display refresh rate.
pub struct SpectrumAnalyzer {
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
    levels: Arc<Mutex<[f32; SPECTRUM_BANDS]>>,
}

impl Default for SpectrumAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl SpectrumAnalyzer {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(true)),
            join: None,
            levels: Arc::new(Mutex::new([0.08; SPECTRUM_BANDS])),
        }
    }

    pub fn levels(&self) -> [f32; SPECTRUM_BANDS] {
        *self.levels.lock()
    }

    pub fn start(&mut self, url: String, title_tx: Option<mpsc::Sender<String>>) {
        self.stop_async();
        *self.levels.lock() = [0.08; SPECTRUM_BANDS];
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Arc::clone(&stop);
        let levels = Arc::clone(&self.levels);
        self.join = Some(thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                if let Err(e) = analyze_direct(&url, Arc::clone(&levels), &stop, title_tx.as_ref())
                {
                    log::debug!("spectrum: {e}");
                }
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let until = Instant::now() + Duration::from_secs(2);
                while Instant::now() < until {
                    if stop.load(Ordering::SeqCst) {
                        *levels.lock() = [0.08; SPECTRUM_BANDS];
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
            *levels.lock() = [0.08; SPECTRUM_BANDS];
        }));
    }

    pub fn stop_async(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            drop(j);
        }
        *self.levels.lock() = [0.08; SPECTRUM_BANDS];
    }
}

impl Drop for SpectrumAnalyzer {
    fn drop(&mut self) {
        self.stop_async();
    }
}

fn analyze_direct(
    url: &str,
    levels: Arc<Mutex<[f32; SPECTRUM_BANDS]>>,
    stop: &Arc<AtomicBool>,
    title_tx: Option<&mpsc::Sender<String>>,
) -> Result<(), String> {
    let sink_rate = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let sink_ch = Arc::new(std::sync::atomic::AtomicU32::new(2));
    crate::audio::decode::run_live_decode_f32(
        url,
        stop,
        title_tx.cloned(),
        Some(levels),
        sink_rate,
        sink_ch,
        |_| {},
    )
}
