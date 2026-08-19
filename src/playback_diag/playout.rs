//! Playout thread pending buffer metrics.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub(crate) static PLAYOUT_PENDING: AtomicU32 = AtomicU32::new(0);
pub(crate) static PLAYOUT_PENDING_MAX: AtomicU32 = AtomicU32::new(0);
pub(crate) static PLAYOUT_TICKS: AtomicU64 = AtomicU64::new(0);
pub(crate) static PLAYOUT_TO_RING: AtomicU64 = AtomicU64::new(0);

pub fn playout_pending(samples: usize) {
    if !super::enabled() {
        return;
    }
    let v = samples.min(u32::MAX as usize) as u32;
    PLAYOUT_PENDING.store(v, Ordering::Relaxed);
    let mut cur_max = PLAYOUT_PENDING_MAX.load(Ordering::Relaxed);
    while v > cur_max {
        match PLAYOUT_PENDING_MAX.compare_exchange_weak(
            cur_max,
            v,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(x) => cur_max = x,
        }
    }
}

pub fn playout_tick(emitted_samples: usize) {
    if !super::enabled() {
        return;
    }
    PLAYOUT_TICKS.fetch_add(1, Ordering::Relaxed);
    PLAYOUT_TO_RING.fetch_add(emitted_samples as u64, Ordering::Relaxed);
}
