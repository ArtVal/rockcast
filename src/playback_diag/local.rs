//! Local cpal ring and PCM queue metrics.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::{event, update_min_max};

pub(crate) static LOCAL_RING: AtomicU32 = AtomicU32::new(0);
pub(crate) static LOCAL_RING_MIN: AtomicU32 = AtomicU32::new(u32::MAX);
pub(crate) static LOCAL_RING_MAX: AtomicU32 = AtomicU32::new(0);
pub(crate) static LOCAL_UNDERRUNS: AtomicU64 = AtomicU64::new(0);
pub(crate) static LOCAL_UNDERRUN_SAMPLES: AtomicU64 = AtomicU64::new(0);
pub(crate) static LOCAL_PCM_Q: AtomicU32 = AtomicU32::new(0);
pub(crate) static LOCAL_PCM_Q_MAX: AtomicU32 = AtomicU32::new(0);
pub(crate) static LOCAL_PCM_Q_FULL: AtomicU64 = AtomicU64::new(0);

pub fn local_ring_fill(samples: usize) {
    if !super::enabled() {
        return;
    }
    let v = samples.min(u32::MAX as usize) as u32;
    LOCAL_RING.store(v, Ordering::Relaxed);
    update_min_max(&LOCAL_RING_MIN, &LOCAL_RING_MAX, v);
}

pub fn local_underrun(missing_samples: usize) {
    if !super::enabled() {
        return;
    }
    let n = LOCAL_UNDERRUNS.fetch_add(1, Ordering::Relaxed) + 1;
    LOCAL_UNDERRUN_SAMPLES.fetch_add(missing_samples as u64, Ordering::Relaxed);
    let ring = LOCAL_RING.load(Ordering::Relaxed);
    if n == 1 || n.is_multiple_of(25) {
        event(
            "local_underrun",
            &format!("count={n} ring={ring} missing={missing_samples}"),
        );
    }
}

pub fn local_pcm_sent() {
    if !super::enabled() {
        return;
    }
    let q = LOCAL_PCM_Q.fetch_add(1, Ordering::Relaxed) + 1;
    let mut cur_max = LOCAL_PCM_Q_MAX.load(Ordering::Relaxed);
    while q > cur_max {
        match LOCAL_PCM_Q_MAX.compare_exchange_weak(
            cur_max,
            q,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(v) => cur_max = v,
        }
    }
}

pub fn local_pcm_recv() {
    if !super::enabled() {
        return;
    }
    LOCAL_PCM_Q.fetch_sub(1, Ordering::Relaxed);
}

pub fn local_pcm_queue_full() {
    if !super::enabled() {
        return;
    }
    let n = LOCAL_PCM_Q_FULL.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 || n.is_multiple_of(10) {
        event("local_pcm_queue_full", &format!("count={n}"));
    }
}
