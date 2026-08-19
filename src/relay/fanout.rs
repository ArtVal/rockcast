use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use parking_lot::{Condvar, Mutex};

use crate::audio::spectrum::BANDS;
use crate::playback_diag;

pub const RING_MAX: usize = 2 * 1024 * 1024;
/// Bytes to keep behind the live edge when a Cast client joins (~0.75 s @ 48 kHz stereo).
pub const JOIN_CUSHION: usize = 144 * 1024;

pub struct Fanout {
    pub stop: Arc<AtomicBool>,
    inner: Mutex<FanoutInner>,
    pub cv: Condvar,
    pub written: AtomicU64,
    title: Mutex<Option<String>>,
    pcm_format: Mutex<Option<(u32, u16)>>,
    levels: Arc<Mutex<[f32; BANDS]>>,
}

struct FanoutInner {
    buf: VecDeque<u8>,
    start: u64,
    ended: bool,
    error: Option<String>,
}

impl Fanout {
    pub fn new(stop: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self {
            stop,
            inner: Mutex::new(FanoutInner {
                buf: VecDeque::with_capacity(RING_MAX),
                start: 0,
                ended: false,
                error: None,
            }),
            cv: Condvar::new(),
            written: AtomicU64::new(0),
            title: Mutex::new(None),
            pcm_format: Mutex::new(None),
            levels: Arc::new(Mutex::new([0.08; BANDS])),
        })
    }

    pub fn levels(&self) -> Arc<Mutex<[f32; BANDS]>> {
        Arc::clone(&self.levels)
    }

    pub fn snapshot_levels(&self) -> [f32; BANDS] {
        *self.levels.lock()
    }

    pub fn set_pcm_format(&self, sample_rate: u32, channels: u16) {
        let mut g = self.pcm_format.lock();
        if g.is_none() {
            *g = Some((sample_rate, channels));
            log::info!("StreamRelay PCM format: {sample_rate} Hz, {channels} ch");
        }
    }

    pub fn pcm_format(&self) -> Option<(u32, u16)> {
        *self.pcm_format.lock()
    }

    pub fn buffered_bytes(&self) -> usize {
        self.inner.lock().buf.len()
    }

    /// When the relay ring is full, pace decode to ~realtime so we don't spin decode+drop.
    pub fn pace_if_full(&self) {
        if self.buffered_bytes() >= RING_MAX {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn push(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let frame_align = self
            .pcm_format()
            .map(|(rate, ch)| pcm_frame_bytes(rate, ch))
            .unwrap_or(4)
            .max(4);
        let mut g = self.inner.lock();
        let mut dropped = 0usize;
        let incoming = data.len();
        if g.buf.len() + incoming > RING_MAX {
            let excess = g.buf.len() + incoming - RING_MAX;
            let drop = ((excess / frame_align).max(1)) * frame_align;
            let drop = drop.min(g.buf.len());
            if drop > 0 {
                g.buf.drain(..drop);
                g.start += drop as u64;
                dropped += drop;
            }
        }
        g.buf.extend(data.iter().copied());
        debug_assert!(g.buf.len() <= RING_MAX);
        self.written
            .store(g.start + g.buf.len() as u64, Ordering::SeqCst);
        let len = g.buf.len();
        drop(g);
        playback_diag::relay_buffer_bytes(len);
        if dropped > 0 {
            playback_diag::relay_dropped(dropped);
        }
        self.cv.notify_all();
    }

    pub fn set_title(&self, title: String) {
        let mut g = self.title.lock();
        if g.as_ref() != Some(&title) {
            *g = Some(title);
        }
    }

    pub fn take_title(&self) -> Option<String> {
        self.title.lock().clone()
    }

    pub fn finish_ok(&self) {
        self.inner.lock().ended = true;
        self.cv.notify_all();
    }

    pub fn finish_err(&self, msg: String) {
        {
            let mut g = self.inner.lock();
            g.ended = true;
            g.error = Some(msg);
        }
        self.cv.notify_all();
    }

    pub fn read_at(&self, pos: &mut u64, out: &mut [u8]) -> Result<usize, String> {
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return Err("stopped".into());
            }
            let mut g = self.inner.lock();
            if let Some(err) = g.error.as_ref() {
                return Err(err.clone());
            }
            let end = g.start + g.buf.len() as u64;
            if *pos < g.start {
                let cushion = (JOIN_CUSHION as u64).min(g.buf.len() as u64 / 2);
                *pos = end.saturating_sub(cushion).max(g.start);
                let align = pcm_sample_bytes(self.pcm_format());
                let off = *pos - g.start;
                *pos = g.start + (off / align) * align;
            }
            if *pos < end {
                let off = (*pos - g.start) as usize;
                let n = (g.buf.len() - off).min(out.len());
                let align = pcm_sample_bytes(self.pcm_format()) as usize;
                let n = n - (n % align);
                if n == 0 {
                    if g.ended {
                        return Ok(0);
                    }
                    let wait_start = std::time::Instant::now();
                    let _ = self.cv.wait_for(&mut g, Duration::from_millis(200));
                    playback_diag::relay_read_wait(wait_start.elapsed());
                    continue;
                }
                let buf = g.buf.make_contiguous();
                out[..n].copy_from_slice(&buf[off..off + n]);
                *pos += n as u64;
                drop(g);
                playback_diag::relay_served(n);
                self.cv.notify_all();
                return Ok(n);
            }
            if g.ended {
                return Ok(0);
            }
            let wait_start = std::time::Instant::now();
            let _ = self.cv.wait_for(&mut g, Duration::from_millis(200));
            playback_diag::relay_read_wait(wait_start.elapsed());
        }
    }

    pub fn copy_at(&self, pos: u64, out: &mut [u8]) -> usize {
        let g = self.inner.lock();
        let end = g.start + g.buf.len() as u64;
        if pos >= end {
            return 0;
        }
        let off = (pos - g.start) as usize;
        let n = (g.buf.len() - off).min(out.len());
        for (i, b) in g.buf.iter().skip(off).take(n).enumerate() {
            out[i] = *b;
        }
        n
    }

    pub fn align_mp3_pos(&self, pos: u64) -> u64 {
        let g = self.inner.lock();
        let end = g.start + g.buf.len() as u64;
        let pos = pos.clamp(g.start, end);
        let off = (pos - g.start) as usize;
        let bytes: Vec<u8> = g.buf.iter().skip(off).copied().collect();
        let Some(rel) = find_mpeg_frame_sync(&bytes) else {
            return pos;
        };
        pos + rel as u64
    }

    pub fn join_position(&self) -> u64 {
        let g = self.inner.lock();
        let end = g.start + g.buf.len() as u64;
        let cushion = (JOIN_CUSHION as u64).min(g.buf.len() as u64);
        let pos = end.saturating_sub(cushion).max(g.start);
        drop(g);
        self.align_pcm_pos(pos)
    }

    pub fn align_pcm_pos(&self, pos: u64) -> u64 {
        let g = self.inner.lock();
        let end = g.start + g.buf.len() as u64;
        if pos >= end {
            return end;
        }
        let align = pcm_sample_bytes(self.pcm_format());
        let off = pos.saturating_sub(g.start);
        g.start + (off / align) * align
    }
}

fn pcm_sample_bytes(format: Option<(u32, u16)>) -> u64 {
    format
        .map(|(_, ch)| u64::from(ch.max(1)) * 2)
        .unwrap_or(4)
        .max(4)
}

fn find_mpeg_frame_sync(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|w| {
        let b0 = w[0];
        let b1 = w[1];
        b0 == 0xFF && (b1 & 0xE0) == 0xE0 && (b1 & 0x18) != 0x08
    })
}

fn pcm_frame_bytes(sample_rate: u32, channels: u16) -> usize {
    let channels = usize::from(channels.max(1));
    ((sample_rate as usize * channels * 2 * 20) / 1000).max(channels * 2 * 256)
}
