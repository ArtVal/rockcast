//! Playback pipeline diagnostics (`ROCKCAST_METRICS=1` / `ROCKCAST_PROFILE=1`).
//!
//! Emits `PLAYBACK_DIAG` summary lines and immediate `DIAG …` warnings into `rockcast.log`.

mod http;
mod local;
mod playout;
mod relay;

use std::{
    sync::atomic::AtomicU32,
    time::{Duration, Instant},
};

use crate::{profile, telemetry::PlaybackSnapshot};

pub(crate) static LAST_LOG: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);

pub fn enabled() -> bool {
    profile::enabled()
}

pub(crate) fn update_min_max(min: &AtomicU32, max: &AtomicU32, value: u32) {
    use std::sync::atomic::Ordering;
    if !enabled() {
        return;
    }
    let mut cur_min = min.load(Ordering::Relaxed);
    while value < cur_min {
        match min.compare_exchange_weak(cur_min, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(v) => cur_min = v,
        }
    }
    let mut cur_max = max.load(Ordering::Relaxed);
    while value > cur_max {
        match max.compare_exchange_weak(cur_max, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(v) => cur_max = v,
        }
    }
}

pub fn event(tag: &str, detail: &str) {
    if enabled() {
        log::warn!("DIAG {tag} {detail}");
    }
}

pub fn maybe_log(snap: PlaybackSnapshot) {
    use std::sync::atomic::Ordering;

    if !enabled() {
        return;
    }
    if !snap.playing {
        reset_session();
        return;
    }

    let mut last = LAST_LOG.lock().unwrap();
    let now = Instant::now();
    if last
        .map(|t| now.duration_since(t) < Duration::from_secs(2))
        .unwrap_or(false)
    {
        return;
    }
    *last = Some(now);
    drop(last);

    let ring = local::LOCAL_RING.load(Ordering::Relaxed);
    let ring_min = local::LOCAL_RING_MIN.load(Ordering::Relaxed);
    let ring_max = local::LOCAL_RING_MAX.load(Ordering::Relaxed);
    let underruns = local::LOCAL_UNDERRUNS.swap(0, Ordering::Relaxed);
    let underrun_samples = local::LOCAL_UNDERRUN_SAMPLES.swap(0, Ordering::Relaxed);
    let pcm_q = local::LOCAL_PCM_Q.load(Ordering::Relaxed);
    let pcm_q_max = local::LOCAL_PCM_Q_MAX.swap(0, Ordering::Relaxed);
    let pcm_q_full = local::LOCAL_PCM_Q_FULL.swap(0, Ordering::Relaxed);
    let playout_pending = playout::PLAYOUT_PENDING.load(Ordering::Relaxed);
    let playout_pending_max = playout::PLAYOUT_PENDING_MAX.swap(0, Ordering::Relaxed);
    let playout_ticks = playout::PLAYOUT_TICKS.swap(0, Ordering::Relaxed);
    let playout_to_ring = playout::PLAYOUT_TO_RING.swap(0, Ordering::Relaxed);

    let relay_buf = relay::RELAY_BUF.load(Ordering::Relaxed);
    let relay_min = relay::RELAY_BUF_MIN.load(Ordering::Relaxed);
    let relay_max = relay::RELAY_BUF_MAX.load(Ordering::Relaxed);
    let relay_drop = relay::RELAY_DROP_BYTES.swap(0, Ordering::Relaxed);
    let relay_wait_slow = relay::RELAY_READ_WAIT_SLOW.swap(0, Ordering::Relaxed);
    let relay_served = relay::RELAY_SERVED_BYTES.swap(0, Ordering::Relaxed);

    let http_gap_max = http::HTTP_GAP_MAX_MS.swap(0, Ordering::Relaxed);
    let http_gap_n = http::HTTP_GAP_OVER_50MS.swap(0, Ordering::Relaxed);
    let http_bytes = http::HTTP_BYTES.swap(0, Ordering::Relaxed);
    let decode_samples = http::DECODE_PCM_SAMPLES.swap(0, Ordering::Relaxed);

    log::info!(
        "PLAYBACK_DIAG local={} relay={} eq={} \
         local_ring={ring}/{ring_min}-{ring_max} underruns={underruns}({underrun_samples}smpl) \
         pcm_q={pcm_q}(max={pcm_q_max} full={pcm_q_full}) \
         playout_pend={playout_pending}(max={playout_pending_max}) playout_ticks={playout_ticks} to_ring={playout_to_ring} \
         relay_buf={relay_buf}/{relay_min}-{relay_max} drop={relay_drop} read_wait_slow={relay_wait_slow} served={relay_served} \
         http_gap_max_ms={http_gap_max} http_gap_n={http_gap_n} http_bytes={http_bytes} decode_smpl={decode_samples}",
        u8::from(snap.playing_local),
        u8::from(snap.cast_relay),
        u8::from(snap.eq_enabled),
    );

    local::LOCAL_RING_MIN.store(ring, Ordering::Relaxed);
    local::LOCAL_RING_MAX.store(ring, Ordering::Relaxed);
    relay::RELAY_BUF_MIN.store(relay_buf, Ordering::Relaxed);
    relay::RELAY_BUF_MAX.store(relay_buf, Ordering::Relaxed);
}

fn reset_session() {
    use std::sync::atomic::Ordering;

    *LAST_LOG.lock().unwrap() = None;
    local::LOCAL_RING.store(0, Ordering::Relaxed);
    local::LOCAL_RING_MIN.store(u32::MAX, Ordering::Relaxed);
    local::LOCAL_RING_MAX.store(0, Ordering::Relaxed);
    local::LOCAL_PCM_Q_MAX.store(0, Ordering::Relaxed);
    playout::PLAYOUT_PENDING.store(0, Ordering::Relaxed);
    playout::PLAYOUT_PENDING_MAX.store(0, Ordering::Relaxed);
    relay::RELAY_BUF.store(0, Ordering::Relaxed);
    relay::RELAY_BUF_MIN.store(u32::MAX, Ordering::Relaxed);
    relay::RELAY_BUF_MAX.store(0, Ordering::Relaxed);
    let _ = local::LOCAL_UNDERRUNS.swap(0, Ordering::Relaxed);
    let _ = local::LOCAL_UNDERRUN_SAMPLES.swap(0, Ordering::Relaxed);
    let _ = local::LOCAL_PCM_Q_FULL.swap(0, Ordering::Relaxed);
    let _ = playout::PLAYOUT_TICKS.swap(0, Ordering::Relaxed);
    let _ = playout::PLAYOUT_TO_RING.swap(0, Ordering::Relaxed);
    let _ = relay::RELAY_DROP_BYTES.swap(0, Ordering::Relaxed);
    let _ = relay::RELAY_READ_WAIT_SLOW.swap(0, Ordering::Relaxed);
    let _ = relay::RELAY_SERVED_BYTES.swap(0, Ordering::Relaxed);
    let _ = http::HTTP_GAP_MAX_MS.swap(0, Ordering::Relaxed);
    let _ = http::HTTP_GAP_OVER_50MS.swap(0, Ordering::Relaxed);
    let _ = http::HTTP_BYTES.swap(0, Ordering::Relaxed);
    let _ = http::DECODE_PCM_SAMPLES.swap(0, Ordering::Relaxed);
}

pub use http::{decode_pcm, http_read};
pub use local::{
    local_pcm_queue_full, local_pcm_recv, local_pcm_sent, local_ring_fill, local_underrun,
};
pub use playout::{playout_pending, playout_tick};
pub use relay::{relay_buffer_bytes, relay_dropped, relay_read_wait, relay_served};
