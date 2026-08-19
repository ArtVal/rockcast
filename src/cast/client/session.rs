//! Cast session state and DMR parsing helpers.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use super::super::channel::{CastChannel, DEFAULT_MEDIA_RECEIVER, NS_MEDIA};

pub(super) struct LiveSession {
    pub channel: Arc<CastChannel>,
    pub stop_hb: Arc<AtomicBool>,
    pub hb: Option<thread::JoinHandle<()>>,
    pub transport_id: String,
    pub session_id: String,
    pub media_session_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoadProgress {
    Pending,
    Ready(Option<i64>),
}

impl LiveSession {
    pub fn start_heartbeat(channel: Arc<CastChannel>) -> (Arc<AtomicBool>, thread::JoinHandle<()>) {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_c = Arc::clone(&stop);
        let ch = Arc::clone(&channel);
        let hb = thread::spawn(move || {
            let mut last_ping = Instant::now();
            while !stop_c.load(Ordering::SeqCst) {
                if last_ping.elapsed() > Duration::from_secs(5) {
                    let _ = ch.ping();
                    last_ping = Instant::now();
                }
                let _ = ch.pump_heartbeats();
                thread::sleep(Duration::from_millis(250));
            }
        });
        (stop, hb)
    }

    pub fn stop_heartbeat(&mut self) {
        self.stop_hb.store(true, Ordering::SeqCst);
        if let Some(h) = self.hb.take() {
            let _ = h.join();
        }
    }
}

#[derive(Clone)]
pub(super) struct AppSession {
    pub transport_id: String,
    pub session_id: String,
}

pub(super) fn parse_dmr(v: &serde_json::Value) -> Option<AppSession> {
    let apps = v.pointer("/status/applications")?.as_array()?;
    for app in apps {
        let app_id = app.get("appId")?.as_str()?;
        if app_id == DEFAULT_MEDIA_RECEIVER {
            return Some(AppSession {
                transport_id: app.get("transportId")?.as_str()?.to_string(),
                session_id: app.get("sessionId")?.as_str()?.to_string(),
            });
        }
    }
    for app in apps {
        let namespaces = app
            .get("namespaces")
            .and_then(|n| n.as_array())
            .cloned()
            .unwrap_or_default();
        let has_media = namespaces.iter().any(|n| {
            n.get("name")
                .and_then(|x| x.as_str())
                .is_some_and(|s| s == NS_MEDIA)
                || n.as_str() == Some(NS_MEDIA)
        });
        if has_media {
            return Some(AppSession {
                transport_id: app.get("transportId")?.as_str()?.to_string(),
                session_id: app.get("sessionId")?.as_str()?.to_string(),
            });
        }
    }
    None
}
