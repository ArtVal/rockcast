//! Optional hot-path timers enabled with `ROCKCAST_PROFILE=1`.

use std::{
    collections::HashMap,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use parking_lot::Mutex;

struct Counter {
    ns: AtomicU64,
    calls: AtomicU64,
}

impl Counter {
    fn record(&self, ns: u64) {
        self.ns.fetch_add(ns, Ordering::Relaxed);
        self.calls.fetch_add(1, Ordering::Relaxed);
    }
}

static ENABLED: OnceLock<bool> = OnceLock::new();
static COUNTERS: OnceLock<Mutex<HashMap<&'static str, Counter>>> = OnceLock::new();
static WORKERS: OnceLock<Mutex<HashMap<&'static str, i64>>> = OnceLock::new();

fn counters() -> &'static Mutex<HashMap<&'static str, Counter>> {
    COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn workers() -> &'static Mutex<HashMap<&'static str, i64>> {
    WORKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var("ROCKCAST_PROFILE")
            .ok()
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            || std::env::var("ROCKCAST_METRICS")
                .ok()
                .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    })
}

pub fn record(label: &'static str, ns: u64) {
    if !enabled() {
        return;
    }
    let mut map = counters().lock();
    map.entry(label)
        .or_insert_with(|| Counter {
            ns: AtomicU64::new(0),
            calls: AtomicU64::new(0),
        })
        .record(ns);
}

pub fn bump(label: &'static str) {
    record(label, 0);
}

/// Registers a long-lived worker while it is executing.  This is deliberately
/// opt-in and only active in diagnostic runs, so production playback has no
/// shared-counter work in its hot paths.
pub fn worker(label: &'static str) -> Option<WorkerGuard> {
    if !enabled() {
        return None;
    }
    let active = {
        let mut map = workers().lock();
        let active = map.entry(label).or_default();
        *active += 1;
        *active
    };
    log::debug!("LIFECYCLE worker_start kind={label} active={active}");
    Some(WorkerGuard { label })
}

pub struct WorkerGuard {
    label: &'static str,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        let active = {
            let mut map = workers().lock();
            let active = map.entry(self.label).or_default();
            *active = active.saturating_sub(1);
            *active
        };
        log::debug!("LIFECYCLE worker_stop kind={} active={active}", self.label);
    }
}

/// Stable, compact active-worker gauges for `METRICS` and soak artefacts.
pub fn worker_snapshot_line() -> String {
    if !enabled() {
        return String::new();
    }
    let mut rows: Vec<_> = workers().lock().iter().map(|(k, v)| (*k, *v)).collect();
    rows.sort_unstable_by_key(|(label, _)| *label);
    rows.into_iter()
        .map(|(label, active)| format!("{label}={active}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn scoped(label: &'static str) -> Option<ProfileGuard> {
    enabled().then(|| ProfileGuard {
        label,
        started: Instant::now(),
    })
}

pub struct ProfileGuard {
    label: &'static str,
    started: Instant,
}

impl Drop for ProfileGuard {
    fn drop(&mut self) {
        record(self.label, self.elapsed().as_nanos() as u64);
    }
}

impl ProfileGuard {
    pub fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }
}

pub fn reset() {
    if let Some(map) = COUNTERS.get() {
        map.lock().clear();
    }
}

/// Compact `key=ms` pairs for the top counters since the last reset.
pub fn snapshot_line() -> String {
    if !enabled() {
        return String::new();
    }
    let map = counters().lock();
    if map.is_empty() {
        return String::new();
    }
    let mut rows: Vec<_> = map
        .iter()
        .map(|(label, counter)| {
            let ns = counter.ns.load(Ordering::Relaxed);
            let calls = counter.calls.load(Ordering::Relaxed);
            (*label, ns, calls)
        })
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.cmp(&a.2)));
    rows.into_iter()
        .take(8)
        .map(|(label, ns, calls)| format!("{label}={:.1}ms/{calls}", ns as f64 / 1e6))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn report(prefix: &str) {
    if !enabled() {
        return;
    }
    let map = counters().lock();
    if map.is_empty() {
        eprintln!("{prefix} profile: no samples");
        return;
    }
    let mut rows: Vec<_> = map
        .iter()
        .map(|(label, counter)| {
            let ns = counter.ns.load(Ordering::Relaxed);
            let calls = counter.calls.load(Ordering::Relaxed);
            (*label, ns, calls)
        })
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    let total_ns: u64 = rows.iter().map(|(_, ns, _)| ns).sum();
    eprintln!(
        "{prefix} profile breakdown (total {:.1} ms):",
        total_ns as f64 / 1e6
    );
    for (label, ns, calls) in rows {
        let pct = if total_ns == 0 {
            0.0
        } else {
            100.0 * ns as f64 / total_ns as f64
        };
        let avg_us = if calls == 0 {
            0.0
        } else {
            ns as f64 / calls as f64 / 1e3
        };
        eprintln!(
            "  {label:24} {pct:5.1}%  {calls:8} calls  avg {avg_us:8.1} us  total {:.1} ms",
            ns as f64 / 1e6
        );
    }
}
