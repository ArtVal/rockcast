//! Playback pipeline diagnostics (`ROCKCAST_METRICS=1` / `ROCKCAST_PROFILE=1`).
//!
//! Emits `PLAYBACK_DIAG` summary lines and immediate `DIAG …` warnings into `rockcast.log`.

use std::{
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
    time::{Duration, Instant},
};

use crate::{profile, telemetry::PlaybackSnapshot};

static LAST_LOG: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);

static LOCAL_RING: AtomicU32 = AtomicU32::new(0);
static LOCAL_RING_MIN: AtomicU32 = AtomicU32::new(u32::MAX);
static LOCAL_RING_MAX: AtomicU32 = AtomicU32::new(0);
static LOCAL_UNDERRUNS: AtomicU64 = AtomicU64::new(0);
static LOCAL_UNDERRUN_SAMPLES: AtomicU64 = AtomicU64::new(0);
static LOCAL_PCM_Q: AtomicU32 = AtomicU32::new(0);
static LOCAL_PCM_Q_MAX: AtomicU32 = AtomicU32::new(0);
static LOCAL_PCM_Q_FULL: AtomicU64 = AtomicU64::new(0);

static PLAYOUT_PENDING: AtomicU32 = AtomicU32::new(0);
static PLAYOUT_PENDING_MAX: AtomicU32 = AtomicU32::new(0);
static PLAYOUT_TICKS: AtomicU64 = AtomicU64::new(0);
static PLAYOUT_TO_RING: AtomicU64 = AtomicU64::new(0);

static RELAY_BUF: AtomicU32 = AtomicU32::new(0);
static RELAY_BUF_MIN: AtomicU32 = AtomicU32::new(u32::MAX);
static RELAY_BUF_MAX: AtomicU32 = AtomicU32::new(0);
static RELAY_DROP_BYTES: AtomicU64 = AtomicU64::new(0);
static RELAY_READ_WAIT_SLOW: AtomicU64 = AtomicU64::new(0);
static RELAY_SERVED_BYTES: AtomicU64 = AtomicU64::new(0);

static HTTP_GAP_MAX_MS: AtomicU32 = AtomicU32::new(0);
static HTTP_GAP_OVER_50MS: AtomicU64 = AtomicU64::new(0);
static HTTP_BYTES: AtomicU64 = AtomicU64::new(0);
static DECODE_PCM_SAMPLES: AtomicU64 = AtomicU64::new(0);

pub fn enabled() -> bool {
    profile::enabled()
}

fn update_min_max(min: &AtomicU32, max: &AtomicU32, value: u32) {
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

pub fn local_ring_fill(samples: usize) {
    if !enabled() {
        return;
    }
    let v = samples.min(u32::MAX as usize) as u32;
    LOCAL_RING.store(v, Ordering::Relaxed);
    update_min_max(&LOCAL_RING_MIN, &LOCAL_RING_MAX, v);
}

pub fn local_underrun(missing_samples: usize) {
    if !enabled() {
        return;
    }
    let n = LOCAL_UNDERRUNS.fetch_add(1, Ordering::Relaxed) + 1;
    LOCAL_UNDERRUN_SAMPLES.fetch_add(missing_samples as u64, Ordering::Relaxed);
    let ring = LOCAL_RING.load(Ordering::Relaxed);
    if n == 1 || n % 25 == 0 {
        event(
            "local_underrun",
            &format!("count={n} ring={ring} missing={missing_samples}"),
        );
    }
}

pub fn local_pcm_sent() {
    if !enabled() {
        return;
    }
    let q = LOCAL_PCM_Q.fetch_add(1, Ordering::Relaxed) + 1;
    let mut cur_max = LOCAL_PCM_Q_MAX.load(Ordering::Relaxed);
    while q > cur_max {
        match LOCAL_PCM_Q_MAX.compare_exchange_weak(cur_max, q, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => break,
            Err(v) => cur_max = v,
        }
    }
}

pub fn local_pcm_recv() {
    if !enabled() {
        return;
    }
    LOCAL_PCM_Q.fetch_sub(1, Ordering::Relaxed);
}

pub fn local_pcm_queue_full() {
    if !enabled() {
        return;
    }
    let n = LOCAL_PCM_Q_FULL.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 || n % 10 == 0 {
        event("local_pcm_queue_full", &format!("count={n}"));
    }
}

pub fn playout_pending(samples: usize) {
    if !enabled() {
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
    if !enabled() {
        return;
    }
    PLAYOUT_TICKS.fetch_add(1, Ordering::Relaxed);
    PLAYOUT_TO_RING.fetch_add(emitted_samples as u64, Ordering::Relaxed);
}

pub fn relay_buffer_bytes(bytes: usize) {
    if !enabled() {
        return;
    }
    let v = bytes.min(u32::MAX as usize) as u32;
    RELAY_BUF.store(v, Ordering::Relaxed);
    update_min_max(&RELAY_BUF_MIN, &RELAY_BUF_MAX, v);
}

pub fn relay_dropped(bytes: usize) {
    if !enabled() {
        return;
    }
    RELAY_DROP_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
    event("relay_drop", &format!("bytes={bytes} total_window={}", RELAY_DROP_BYTES.load(Ordering::Relaxed)));
}

pub fn relay_read_wait(d: Duration) {
    if !enabled() {
        return;
    }
    if d >= Duration::from_millis(100) {
        let n = RELAY_READ_WAIT_SLOW.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 3 || n % 20 == 0 {
            event("relay_read_wait", &format!("wait_ms={} count={n}", d.as_millis()));
        }
    }
}

pub fn relay_served(bytes: usize) {
    if !enabled() {
        return;
    }
    RELAY_SERVED_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
}

pub fn http_read(gap: Duration, bytes: usize) {
    if !enabled() {
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
    if !enabled() {
        return;
    }
    DECODE_PCM_SAMPLES.fetch_add(samples as u64, Ordering::Relaxed);
}

pub fn maybe_log(snap: PlaybackSnapshot) {
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

    let ring = LOCAL_RING.load(Ordering::Relaxed);
    let ring_min = LOCAL_RING_MIN.load(Ordering::Relaxed);
    let ring_max = LOCAL_RING_MAX.load(Ordering::Relaxed);
    let underruns = LOCAL_UNDERRUNS.swap(0, Ordering::Relaxed);
    let underrun_samples = LOCAL_UNDERRUN_SAMPLES.swap(0, Ordering::Relaxed);
    let pcm_q = LOCAL_PCM_Q.load(Ordering::Relaxed);
    let pcm_q_max = LOCAL_PCM_Q_MAX.swap(0, Ordering::Relaxed);
    let pcm_q_full = LOCAL_PCM_Q_FULL.swap(0, Ordering::Relaxed);
    let playout_pending = PLAYOUT_PENDING.load(Ordering::Relaxed);
    let playout_pending_max = PLAYOUT_PENDING_MAX.swap(0, Ordering::Relaxed);
    let playout_ticks = PLAYOUT_TICKS.swap(0, Ordering::Relaxed);
    let playout_to_ring = PLAYOUT_TO_RING.swap(0, Ordering::Relaxed);

    let relay_buf = RELAY_BUF.load(Ordering::Relaxed);
    let relay_min = RELAY_BUF_MIN.load(Ordering::Relaxed);
    let relay_max = RELAY_BUF_MAX.load(Ordering::Relaxed);
    let relay_drop = RELAY_DROP_BYTES.swap(0, Ordering::Relaxed);
    let relay_wait_slow = RELAY_READ_WAIT_SLOW.swap(0, Ordering::Relaxed);
    let relay_served = RELAY_SERVED_BYTES.swap(0, Ordering::Relaxed);

    let http_gap_max = HTTP_GAP_MAX_MS.swap(0, Ordering::Relaxed);
    let http_gap_n = HTTP_GAP_OVER_50MS.swap(0, Ordering::Relaxed);
    let http_bytes = HTTP_BYTES.swap(0, Ordering::Relaxed);
    let decode_samples = DECODE_PCM_SAMPLES.swap(0, Ordering::Relaxed);

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

    LOCAL_RING_MIN.store(ring, Ordering::Relaxed);
    LOCAL_RING_MAX.store(ring, Ordering::Relaxed);
    RELAY_BUF_MIN.store(relay_buf, Ordering::Relaxed);
    RELAY_BUF_MAX.store(relay_buf, Ordering::Relaxed);
}

fn reset_session() {
    *LAST_LOG.lock().unwrap() = None;
    LOCAL_RING.store(0, Ordering::Relaxed);
    LOCAL_RING_MIN.store(u32::MAX, Ordering::Relaxed);
    LOCAL_RING_MAX.store(0, Ordering::Relaxed);
    LOCAL_PCM_Q_MAX.store(0, Ordering::Relaxed);
    PLAYOUT_PENDING.store(0, Ordering::Relaxed);
    PLAYOUT_PENDING_MAX.store(0, Ordering::Relaxed);
    RELAY_BUF.store(0, Ordering::Relaxed);
    RELAY_BUF_MIN.store(u32::MAX, Ordering::Relaxed);
    RELAY_BUF_MAX.store(0, Ordering::Relaxed);
    let _ = LOCAL_UNDERRUNS.swap(0, Ordering::Relaxed);
    let _ = LOCAL_UNDERRUN_SAMPLES.swap(0, Ordering::Relaxed);
    let _ = LOCAL_PCM_Q_FULL.swap(0, Ordering::Relaxed);
    let _ = PLAYOUT_TICKS.swap(0, Ordering::Relaxed);
    let _ = PLAYOUT_TO_RING.swap(0, Ordering::Relaxed);
    let _ = RELAY_DROP_BYTES.swap(0, Ordering::Relaxed);
    let _ = RELAY_READ_WAIT_SLOW.swap(0, Ordering::Relaxed);
    let _ = RELAY_SERVED_BYTES.swap(0, Ordering::Relaxed);
    let _ = HTTP_GAP_MAX_MS.swap(0, Ordering::Relaxed);
    let _ = HTTP_GAP_OVER_50MS.swap(0, Ordering::Relaxed);
    let _ = HTTP_BYTES.swap(0, Ordering::Relaxed);
    let _ = DECODE_PCM_SAMPLES.swap(0, Ordering::Relaxed);
}
