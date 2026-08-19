//! Shared audio-stream helpers used by local playback and spectrum analysis.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use rustfft::{Fft, FftPlanner, num_complex::Complex};
use symphonia::core::probe::Hint;

pub const BANDS: usize = 24;
const FFT_SIZE: usize = 1024;
const HOP: usize = 512;

/// UI/spectrum level publish rate (~50 Hz).
pub(crate) const LEVEL_PUBLISH_INTERVAL: Duration = Duration::from_millis(20);

pub(crate) struct LevelPublisher {
    last: Instant,
    pending: Option<[f32; BANDS]>,
}

impl LevelPublisher {
    pub(crate) fn new() -> Self {
        Self {
            last: Instant::now() - LEVEL_PUBLISH_INTERVAL,
            pending: None,
        }
    }

    pub(crate) fn publish_limited(&mut self, values: [f32; BANDS], write: impl FnOnce([f32; BANDS])) {
        self.pending = Some(values);
        if self.last.elapsed() >= LEVEL_PUBLISH_INTERVAL
            && let Some(values) = self.pending.take()
        {
            write(values);
            self.last = Instant::now();
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

pub(crate) struct BandAnalyzer {
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
    pub(crate) fn new() -> Self {
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

    pub(crate) fn push_mono<I>(&mut self, samples: I, sample_rate: f32) -> Option<[f32; BANDS]>
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

pub(crate) fn apply_hint(hint: &mut Hint, content_type: &str) {
    if content_type.contains("mpeg") || content_type.contains("mp3") {
        hint.with_extension("mp3");
    } else if content_type.contains("aac") || content_type.contains("mp4") {
        hint.with_extension("aac");
    } else if content_type.contains("wav")
        || content_type.contains("wave")
        || content_type.contains("pcm")
    {
        hint.with_extension("wav");
    } else if content_type.contains("ogg") || content_type.contains("vorbis") {
        hint.with_extension("ogg");
    } else if content_type.contains("flac") {
        hint.with_extension("flac");
    } else {
        hint.with_extension("mp3");
    }
}

pub(crate) fn parse_stream_title(meta: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(meta);
    let lower = text.to_ascii_lowercase();
    let start = lower.find("streamtitle='")? + "streamtitle='".len();
    let rest = &text[start..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
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
    use super::{BandAnalyzer, FFT_SIZE, parse_stream_title};
    use std::{thread, time::Duration};

    #[test]
    fn parses_icy_stream_title_case_insensitively() {
        assert_eq!(
            parse_stream_title(b"StreamTitle='Artist - Track';"),
            Some("Artist - Track".into())
        );
    }

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
