//! FFT spectrum bands for EQ visualization.

use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use rustfft::{num_complex::Complex, Fft, FftPlanner};

pub const BANDS: usize = 24;
const FFT_SIZE: usize = 1024;
const HOP: usize = 512;

/// UI/spectrum level publish rate (~50 Hz).
pub const LEVEL_PUBLISH_INTERVAL: Duration = Duration::from_millis(20);

pub struct LevelPublisher {
    last: Instant,
    pending: Option<[f32; BANDS]>,
}

impl LevelPublisher {
    pub fn new() -> Self {
        Self {
            last: Instant::now() - LEVEL_PUBLISH_INTERVAL,
            pending: None,
        }
    }

    pub fn publish_limited(&mut self, values: [f32; BANDS], write: impl FnOnce([f32; BANDS])) {
        self.pending = Some(values);
        if self.last.elapsed() >= LEVEL_PUBLISH_INTERVAL
            && let Some(values) = self.pending.take()
        {
            write(values);
            self.last = Instant::now();
        }
    }
}

impl Default for LevelPublisher {
    fn default() -> Self {
        Self::new()
    }
}

/// FFT analyzer wired to a shared levels buffer (playout / relay decode at device rate).
pub struct SpectrumTap {
    bands: BandAnalyzer,
    publish: LevelPublisher,
    levels: Arc<Mutex<[f32; BANDS]>>,
    mono_buf: Vec<f32>,
}

impl SpectrumTap {
    pub fn new(levels: Arc<Mutex<[f32; BANDS]>>) -> Self {
        Self {
            bands: BandAnalyzer::new(),
            publish: LevelPublisher::new(),
            levels,
            mono_buf: Vec::with_capacity(4096),
        }
    }

    pub fn push_f32(&mut self, pcm: &[f32], channels: usize, sample_rate: u32) {
        let ch = channels.max(1);
        self.mono_buf.clear();
        self.mono_buf.reserve(pcm.len() / ch);
        for frame in pcm.chunks(ch) {
            self.mono_buf.push(frame.iter().sum::<f32>() / ch as f32);
        }
        self.analyze(sample_rate);
    }

    /// Interleaved 16-bit LE PCM (relay playout chunks).
    pub fn push_i16_le(&mut self, pcm: &[u8], channels: usize, sample_rate: u32) {
        let ch = channels.max(1);
        let frame_bytes = ch * 2;
        self.mono_buf.clear();
        self.mono_buf.reserve(pcm.len() / frame_bytes);
        for frame in pcm.chunks(frame_bytes) {
            if frame.len() < frame_bytes {
                break;
            }
            let mut sum = 0.0f32;
            for c in 0..ch {
                let off = c * 2;
                let s = i16::from_le_bytes([frame[off], frame[off + 1]]);
                sum += f32::from(s) / f32::from(i16::MAX);
            }
            self.mono_buf.push(sum / ch as f32);
        }
        self.analyze(sample_rate);
    }

    fn analyze(&mut self, sample_rate: u32) {
        if let Some(values) = self
            .bands
            .push_mono(self.mono_buf.iter().copied(), sample_rate as f32)
        {
            self.publish
                .publish_limited(values, |values| *self.levels.lock() = values);
        }
    }
}

#[derive(Clone)]
struct BandRanges {
    bins: [(usize, usize); BANDS],
}

impl BandRanges {
    fn for_rate(sample_rate: f32) -> Self {
        let half = FFT_SIZE / 2;
        let f_min = 40.0f32;
        let f_max = (sample_rate * 0.45).min(16_000.0);
        let ratio = f_max / f_min;
        let mut bins = [(0, 1); BANDS];
        for (band, slot) in bins.iter_mut().enumerate() {
            let lo = f_min * ratio.powf(band as f32 / BANDS as f32);
            let hi = f_min * ratio.powf((band + 1) as f32 / BANDS as f32);
            let i0 = ((lo / sample_rate) * FFT_SIZE as f32).floor() as usize;
            let i1 = ((hi / sample_rate) * FFT_SIZE as f32).ceil() as usize;
            let i0 = i0.min(half - 1);
            *slot = (i0, i1.clamp(i0 + 1, half));
        }
        Self { bins }
    }
}

pub struct BandAnalyzer {
    pcm: VecDeque<f32>,
    fft: std::sync::Arc<dyn Fft<f32>>,
    fft_buf: Vec<Complex<f32>>,
    window: Vec<f32>,
    smooth: [f32; BANDS],
    band_ranges: BandRanges,
    sample_rate: f32,
    last_fft: Instant,
}

impl BandAnalyzer {
    pub fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        Self {
            pcm: VecDeque::with_capacity(FFT_SIZE * 2),
            fft: planner.plan_fft_forward(FFT_SIZE),
            fft_buf: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            window: hann(FFT_SIZE),
            smooth: [0.08; BANDS],
            band_ranges: BandRanges::for_rate(48_000.0),
            sample_rate: 48_000.0,
            last_fft: Instant::now() - LEVEL_PUBLISH_INTERVAL,
        }
    }

    pub fn push_mono<I>(&mut self, samples: I, sample_rate: f32) -> Option<[f32; BANDS]>
    where
        I: IntoIterator<Item = f32>,
    {
        if (self.sample_rate - sample_rate).abs() > 1.0 {
            self.sample_rate = sample_rate;
            self.band_ranges = BandRanges::for_rate(sample_rate);
        }

        self.pcm.extend(samples);
        while self.pcm.len() > FFT_SIZE + HOP {
            self.pcm.drain(..HOP);
        }

        if self.last_fft.elapsed() < LEVEL_PUBLISH_INTERVAL || self.pcm.len() < FFT_SIZE {
            return None;
        }

        {
            let _window = crate::profile::scoped("fft_window");
            for (slot, (sample, window)) in self
                .fft_buf
                .iter_mut()
                .zip(self.pcm.iter().zip(self.window.iter()))
            {
                slot.re = sample * window;
                slot.im = 0.0;
            }
        }
        self.pcm.drain(..HOP);
        {
            let _fft = crate::profile::scoped("fft_process");
            self.fft.process(&mut self.fft_buf);
        }
        let bands = {
            let _bands = crate::profile::scoped("fft_bands");
            magnitudes_to_bands(&self.fft_buf, &self.band_ranges)
        };
        {
            let _smooth = crate::profile::scoped("fft_smooth");
            for (i, band) in bands.iter().enumerate() {
                let rate = if *band > self.smooth[i] { 0.65 } else { 0.25 };
                self.smooth[i] += (*band - self.smooth[i]) * rate;
            }
        }
        self.last_fft = Instant::now();
        Some(self.smooth)
    }
}

impl Default for BandAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let x = std::f32::consts::PI * 2.0 * i as f32 / (n as f32 - 1.0);
            0.5 - 0.5 * x.cos()
        })
        .collect()
}

fn magnitudes_to_bands(fft: &[Complex<f32>], ranges: &BandRanges) -> [f32; BANDS] {
    let scale = FFT_SIZE as f32;
    let mut out = [0.0; BANDS];
    for (band, value) in out.iter_mut().enumerate() {
        let (i0, i1) = ranges.bins[band];
        let peak_sq = fft[i0..i1]
            .iter()
            .map(|c| c.re * c.re + c.im * c.im)
            .fold(0.0f32, f32::max);
        let peak = peak_sq.sqrt() / scale;
        *value = ((20.0 * peak.max(1e-8).log10() + 60.0) / 60.0).clamp(0.0, 1.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{BandAnalyzer, FFT_SIZE};
    use std::{thread, time::Duration};

    #[test]
    fn analyzer_waits_for_a_full_window() {
        let mut analyzer = BandAnalyzer::new();
        assert!(
            analyzer
                .push_mono(vec![0.0; FFT_SIZE - 1], 44_100.0)
                .is_none()
        );
        assert!(analyzer.push_mono([0.0], 44_100.0).is_some());
    }

    #[test]
    fn analyzer_decimates_fft_updates() {
        let mut analyzer = BandAnalyzer::new();
        let first = analyzer.push_mono(vec![0.0; FFT_SIZE], 48_000.0);
        assert!(first.is_some());
        assert!(analyzer.push_mono(vec![0.0; FFT_SIZE], 48_000.0).is_none());
        thread::sleep(Duration::from_millis(25));
        assert!(analyzer.push_mono(vec![0.0; FFT_SIZE], 48_000.0).is_some());
    }
}
