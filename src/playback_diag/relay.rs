//! Cast relay fanout buffer metrics.

use std::{
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
    time::Duration,
};

use super::{event, update_min_max};

pub(crate) static RELAY_BUF: AtomicU32 = AtomicU32::new(0);
pub(crate) static RELAY_BUF_MIN: AtomicU32 = AtomicU32::new(u32::MAX);
pub(crate) static RELAY_BUF_MAX: AtomicU32 = AtomicU32::new(0);
pub(crate) static RELAY_DROP_BYTES: AtomicU64 = AtomicU64::new(0);
pub(crate) static RELAY_READ_WAIT_SLOW: AtomicU64 = AtomicU64::new(0);
pub(crate) static RELAY_SERVED_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn relay_buffer_bytes(bytes: usize) {
    if !super::enabled() {
        return;
    }
    let v = bytes.min(u32::MAX as usize) as u32;
    RELAY_BUF.store(v, Ordering::Relaxed);
    update_min_max(&RELAY_BUF_MIN, &RELAY_BUF_MAX, v);
}

pub fn relay_dropped(bytes: usize) {
    if !super::enabled() {
        return;
    }
    RELAY_DROP_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
}

pub fn relay_read_wait(d: Duration) {
    if !super::enabled() {
        return;
    }
    if d >= Duration::from_millis(100) {
        let n = RELAY_READ_WAIT_SLOW.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 3 || n.is_multiple_of(20) {
            event(
                "relay_read_wait",
                &format!("wait_ms={} count={n}", d.as_millis()),
            );
        }
    }
}

pub fn relay_served(bytes: usize) {
    if !super::enabled() {
        return;
    }
    RELAY_SERVED_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
}
