//! HTTP read gaps and decode throughput.

use std::{
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
    time::Duration,
};

use super::event;

pub(crate) static HTTP_GAP_MAX_MS: AtomicU32 = AtomicU32::new(0);
pub(crate) static HTTP_GAP_OVER_50MS: AtomicU64 = AtomicU64::new(0);
pub(crate) static HTTP_BYTES: AtomicU64 = AtomicU64::new(0);
pub(crate) static DECODE_PCM_SAMPLES: AtomicU64 = AtomicU64::new(0);

pub fn http_read(gap: Duration, bytes: usize) {
    if !super::enabled() {
        return;
    }
    HTTP_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
    if bytes == 0 {
        return;
    }
    let ms = gap.as_millis().min(u32::MAX as u128) as u32;
    let prev = HTTP_GAP_MAX_MS.load(Ordering::Relaxed);
    if ms > prev {
        HTTP_GAP_MAX_MS.store(ms, Ordering::Relaxed);
    }
    if gap >= Duration::from_millis(50) {
        let n = HTTP_GAP_OVER_50MS.fetch_add(1, Ordering::Relaxed) + 1;
        if gap >= Duration::from_millis(150) && (n <= 5 || n % 15 == 0) {
            event(
                "http_read_gap",
                &format!("gap_ms={ms} bytes={bytes} count={n}"),
            );
        }
    }
}

pub fn decode_pcm(samples: usize) {
    if !super::enabled() {
        return;
    }
    DECODE_PCM_SAMPLES.fetch_add(samples as u64, Ordering::Relaxed);
}
