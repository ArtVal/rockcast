//! PCM output smoothing, rate conversion, and clock-paced playout.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const CAST_PCM_RATE: u32 = 48_000;

/// Single-producer / single-consumer float ring (decode → cpal).
pub struct SpscAudioRing {
    buf: Box<[UnsafeCell<f32>]>,
    mask: usize,
    write: AtomicUsize,
    read: AtomicUsize,
}

// SAFETY: one writer (playout thread) and one reader (cpal callback).
unsafe impl Send for SpscAudioRing {}
unsafe impl Sync for SpscAudioRing {}

impl SpscAudioRing {
    pub fn with_capacity(capacity: usize) -> Self {
        let cap = capacity.next_power_of_two().max(4096);
        let mut buf = Vec::with_capacity(cap);
        buf.resize_with(cap, || UnsafeCell::new(0.0));
        Self {
            buf: buf.into_boxed_slice(),
            mask: cap - 1,
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
        }
    }

    fn count(&self) -> usize {
        self.write.load(Ordering::Acquire) - self.read.load(Ordering::Acquire)
    }

    pub fn len(&self) -> usize {
        self.count()
    }

    pub fn clear(&self) {
        let w = self.write.load(Ordering::Relaxed);
        self.read.store(w, Ordering::Release);
    }

    /// Producer: push up to `samples.len()` items.
    pub fn push_slice(&self, samples: &[f32]) -> usize {
        let mut w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);
        let mut n = 0usize;
        while n < samples.len() && w.wrapping_sub(r) < self.buf.len() {
            unsafe {
                *self.buf[w & self.mask].get() = samples[n];
            }
            w = w.wrapping_add(1);
            n += 1;
        }
        self.write.store(w, Ordering::Release);
        n
    }

    /// Consumer: copy up to `out.len()` samples.
    pub fn pop_slice(&self, out: &mut [f32]) -> usize {
        let mut r = self.read.load(Ordering::Relaxed);
        let w = self.write.load(Ordering::Acquire);
        let mut n = 0usize;
        while n < out.len() && r != w {
            out[n] = unsafe { *self.buf[r & self.mask].get() };
            r = r.wrapping_add(1);
            n += 1;
        }
        self.read.store(r, Ordering::Release);
        n
    }
}

/// Emit fixed-size f32 frames on a wall clock (turns bursty decode into steady PCM).
pub struct SteadyPlayout {
    pending: Vec<f32>,
    frame_samples: usize,
    next_tick: Instant,
    tick: Duration,
}

impl SteadyPlayout {
    pub fn new(sample_rate: u32, channels: usize, tick_ms: u32) -> Self {
        let channels = channels.max(1);
        let frame_samples =
            ((sample_rate as usize * channels * tick_ms as usize) / 1000).max(channels);
        Self {
            pending: Vec::new(),
            frame_samples,
            next_tick: Instant::now(),
            tick: Duration::from_millis(tick_ms.max(1) as u64),
        }
    }

    pub fn reset_format(&mut self, sample_rate: u32, channels: usize, tick_ms: u32) {
        let channels = channels.max(1);
        let frame_samples =
            ((sample_rate as usize * channels * tick_ms as usize) / 1000).max(channels);
        if frame_samples != self.frame_samples {
            self.frame_samples = frame_samples;
            self.pending.clear();
            self.next_tick = Instant::now();
        }
    }

    pub fn ingest(&mut self, samples: &[f32]) {
        if !samples.is_empty() {
            self.pending.extend_from_slice(samples);
        }
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Release up to `max_frames` frames whose tick time has arrived.
    pub fn drain_due(&mut self, max_frames: usize) -> Vec<f32> {
        let mut out = Vec::new();
        let mut emitted = 0usize;
        while emitted < max_frames
            && self.pending.len() >= self.frame_samples
            && Instant::now() >= self.next_tick
        {
            out.extend_from_slice(&self.pending[..self.frame_samples]);
            self.pending.drain(..self.frame_samples);
            self.next_tick += self.tick;
            emitted += 1;
        }
        if self.next_tick < Instant::now() - Duration::from_millis(500) {
            self.next_tick = Instant::now();
        }
        out
    }

    pub fn sleep_hint(&self) -> Duration {
        if self.pending.len() >= self.frame_samples {
            self.next_tick
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(20))
        } else {
            Duration::from_millis(5)
        }
    }
}

/// Same as [`SteadyPlayout`] for interleaved 16-bit PCM bytes (relay / Cast).
pub struct SteadyBytePlayout {
    pending: Vec<u8>,
    frame_bytes: usize,
    next_tick: Instant,
    tick: Duration,
}

impl SteadyBytePlayout {
    pub fn new(sample_rate: u32, channels: u16, tick_ms: u32) -> Self {
        let frame_bytes = frame_bytes_for_ms(sample_rate, channels, tick_ms);
        Self {
            pending: Vec::new(),
            frame_bytes,
            next_tick: Instant::now(),
            tick: Duration::from_millis(tick_ms.max(1) as u64),
        }
    }

    pub fn reset_format(&mut self, sample_rate: u32, channels: u16, tick_ms: u32) {
        let frame_bytes = frame_bytes_for_ms(sample_rate, channels, tick_ms);
        if frame_bytes != self.frame_bytes {
            self.frame_bytes = frame_bytes;
            self.pending.clear();
            self.next_tick = Instant::now();
        }
    }

    pub fn ingest(&mut self, pcm: &[u8]) {
        if !pcm.is_empty() {
            self.pending.extend_from_slice(pcm);
        }
    }

    pub fn drain_due(&mut self, max_frames: usize) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut emitted = 0usize;
        while emitted < max_frames
            && self.pending.len() >= self.frame_bytes
            && Instant::now() >= self.next_tick
        {
            out.push(self.pending.drain(..self.frame_bytes).collect());
            self.next_tick += self.tick;
            emitted += 1;
        }
        if self.next_tick < Instant::now() - Duration::from_millis(500) {
            self.next_tick = Instant::now();
        }
        out
    }

    pub fn sleep_hint(&self) -> Duration {
        if self.pending.len() >= self.frame_bytes {
            self.next_tick
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(20))
        } else {
            Duration::from_millis(5)
        }
    }
}

pub struct PcmSmoother {
    buf: Vec<u8>,
    frame_bytes: usize,
}

impl PcmSmoother {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            buf: Vec::new(),
            frame_bytes: frame_bytes_for(sample_rate, channels),
        }
    }

    pub fn set_format(&mut self, sample_rate: u32, channels: u16) {
        self.frame_bytes = frame_bytes_for(sample_rate, channels);
    }

    pub fn push(&mut self, pcm: &[u8], mut emit: impl FnMut(&[u8])) {
        if pcm.is_empty() {
            return;
        }
        self.buf.extend_from_slice(pcm);
        while self.buf.len() >= self.frame_bytes {
            emit(&self.buf[..self.frame_bytes]);
            self.buf.drain(..self.frame_bytes);
        }
    }

    pub fn flush(&mut self, mut emit: impl FnMut(&[u8])) {
        if self.buf.is_empty() {
            return;
        }
        emit(&self.buf);
        self.buf.clear();
    }
}

/// Resample interleaved f32 PCM (keeps state across calls).
pub struct PcmResampler {
    src_rate: u32,
    dst_rate: u32,
    channels: usize,
    src_pos: f64,
    pending: Vec<f32>,
}

/// Alias for relay/Cast paths that target 48 kHz.
pub type CastPcmResampler = PcmResampler;

impl PcmResampler {
    pub fn new(channels: u16) -> Self {
        Self {
            src_rate: 0,
            dst_rate: CAST_PCM_RATE,
            channels: usize::from(channels.max(1)),
            src_pos: 0.0,
            pending: Vec::new(),
        }
    }

    pub fn set_format(&mut self, sample_rate: u32, channels: u16, dst_rate: u32) {
        let channels = usize::from(channels.max(1));
        let dst_rate = dst_rate.max(1);
        if self.src_rate != sample_rate
            || self.channels != channels
            || self.dst_rate != dst_rate
        {
            self.src_rate = sample_rate;
            self.dst_rate = dst_rate;
            self.channels = channels;
            self.src_pos = 0.0;
            self.pending.clear();
        }
    }

    pub fn push(&mut self, pcm: &[f32], mut emit: impl FnMut(&[f32])) {
        if pcm.is_empty() || self.src_rate == 0 {
            return;
        }
        if self.src_rate == self.dst_rate {
            emit(pcm);
            return;
        }
        self.pending.extend_from_slice(pcm);
        let ch = self.channels;
        let ratio = f64::from(self.src_rate) / f64::from(self.dst_rate);
        let mut out = Vec::new();
        loop {
            let need = (self.src_pos.floor() as usize + 2) * ch;
            if self.pending.len() < need {
                break;
            }
            let i0 = self.src_pos.floor() as usize;
            let frac = (self.src_pos - i0 as f64) as f32;
            for c in 0..ch {
                let s0 = self.pending[i0 * ch + c];
                let s1 = self.pending[(i0 + 1) * ch + c];
                out.push(s0 + (s1 - s0) * frac);
            }
            self.src_pos += ratio;
            while self.src_pos >= 1.0 && self.pending.len() >= ch {
                self.pending.drain(..ch);
                self.src_pos -= 1.0;
            }
        }
        if !out.is_empty() {
            emit(&out);
        }
    }
}

pub fn cast_pcm_rate() -> u32 {
    CAST_PCM_RATE
}

pub fn upmix_interleaved(pcm: &[f32], src_ch: usize, dst_ch: usize) -> Vec<f32> {
    if src_ch == dst_ch || src_ch == 0 {
        return pcm.to_vec();
    }
    let src_ch = src_ch.max(1);
    let dst_ch = dst_ch.max(1);
    let frames = pcm.len() / src_ch;
    let mut out = Vec::with_capacity(frames * dst_ch);
    for frame in pcm.chunks(src_ch) {
        for c in 0..dst_ch {
            out.push(frame[c.min(src_ch - 1)]);
        }
    }
    out
}

fn frame_bytes_for(sample_rate: u32, channels: u16) -> usize {
    frame_bytes_for_ms(sample_rate, channels, 20)
}

fn frame_bytes_for_ms(sample_rate: u32, channels: u16, ms: u32) -> usize {
    let channels = usize::from(channels.max(1));
    ((sample_rate as usize * channels * 2 * ms as usize) / 1000).max(channels * 2 * 256)
}
