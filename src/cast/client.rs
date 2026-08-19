//! High-level CASTV2: CONNECT → LAUNCH DMR → LOAD live stream.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use serde_json::json;
use thiserror::Error;

use super::{
    channel::{
        CastChannel, ChannelError, DEFAULT_MEDIA_RECEIVER, NS_CONNECTION, NS_MEDIA, NS_RECEIVER,
        RECEIVER_ID,
    },
    discovery::{DiscoveredDevice, DiscoveryError, discover, discover_streaming},
    proto::Payload,
};

#[derive(Debug, Error)]
pub enum CastError {
    #[error(transparent)]
    Discovery(#[from] DiscoveryError),
    #[error(transparent)]
    Channel(#[from] ChannelError),
}

#[derive(Clone)]
pub struct CastDeviceInfo {
    pub discovered: DiscoveredDevice,
}

impl CastDeviceInfo {
    pub fn label(&self) -> String {
        self.discovered.label()
    }
}

struct LiveSession {
    channel: Arc<CastChannel>,
    stop_hb: Arc<AtomicBool>,
    hb: Option<thread::JoinHandle<()>>,
    transport_id: String,
    session_id: String,
    media_session_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoadProgress {
    Pending,
    Ready(Option<i64>),
}

impl LiveSession {
    fn start_heartbeat(channel: Arc<CastChannel>) -> (Arc<AtomicBool>, thread::JoinHandle<()>) {
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

    fn stop_heartbeat(&mut self) {
        self.stop_hb.store(true, Ordering::SeqCst);
        if let Some(h) = self.hb.take() {
            let _ = h.join();
        }
    }
}

pub struct CastService {
    current: Mutex<Option<(String, LiveSession)>>,
    /// Serializes play/stop so a hung LOAD cannot interleave with the next op.
    op_lock: Mutex<()>,
    /// Set to cancel an in-flight `receive_find` (new play / stop / shutdown).
    cancel: AtomicBool,
}

impl Default for CastService {
    fn default() -> Self {
        Self::new()
    }
}

impl CastService {
    pub fn new() -> Self {
        Self {
            current: Mutex::new(None),
            op_lock: Mutex::new(()),
            cancel: AtomicBool::new(false),
        }
    }

    /// Interrupt an in-flight Cast wait without waiting for the operation lock.
    pub fn cancel_pending(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Scan for Cast devices on the local network.
    pub fn scan(timeout: Duration) -> Result<Vec<CastDeviceInfo>, CastError> {
        let list = discover(timeout)?;
        Ok(list
            .into_iter()
            .map(|d| CastDeviceInfo { discovered: d })
            .collect())
    }

    /// Scan while reporting devices as soon as either discovery path finds them.
    pub fn scan_streaming(
        timeout: Duration,
        mut on_found: impl FnMut(CastDeviceInfo),
    ) -> Result<Vec<CastDeviceInfo>, CastError> {
        let list = discover_streaming(timeout, |discovered| {
            on_found(CastDeviceInfo { discovered });
        })?;
        Ok(list
            .into_iter()
            .map(|discovered| CastDeviceInfo { discovered })
            .collect())
    }

    pub fn play(
        &self,
        device: &CastDeviceInfo,
        url: &str,
        content_type: &str,
        title: &str,
        on_status: impl Fn(&str),
    ) -> Result<(), CastError> {
        // Abort any previous play waiting on LOAD, then take the op lock.
        self.cancel.store(true, Ordering::SeqCst);
        let _op = self.op_lock.lock();
        self.cancel.store(false, Ordering::SeqCst);

        on_status(&format!("Connecting to «{}»…", device.discovered.name));
        log::info!(
            "CastService::play device='{}' url={url}",
            device.discovered.name
        );

        {
            let mut guard = self.current.lock();
            if let Some((id, mut old)) = guard.take()
                && id != device.discovered.id
            {
                old.stop_heartbeat();
                let _ = Self::send_stop(&old);
            } else if let Some(pair) = guard.take() {
                *guard = Some(pair);
            }
        }

        let channel = self.take_or_connect(device)?;

        on_status(&format!("Starting stream: {title}"));

        channel.send_json(
            RECEIVER_ID,
            NS_CONNECTION,
            &json!({ "type": "CONNECT", "userAgent": "RockCast/0.1" }),
        )?;

        let app = self.ensure_media_receiver(&channel)?;
        channel.send_json(
            &app.transport_id,
            NS_CONNECTION,
            &json!({ "type": "CONNECT", "userAgent": "RockCast/0.1" }),
        )?;

        let content_types = candidate_content_types(content_type);

        let mut last_err = None;
        let mut media_session_id = None;
        for ct in &content_types {
            if self.cancel.load(Ordering::SeqCst) {
                return Err(CastError::Channel(ChannelError::Cancelled));
            }
            match Self::load_media(&channel, &app, url, ct, title, &self.cancel) {
                Ok(mid) => {
                    media_session_id = mid;
                    last_err = None;
                    break;
                }
                Err(e) => {
                    log::warn!("CastService::load_media({ct}) failed: {e}");
                    on_status(&format!("Retry with {ct}..."));
                    last_err = Some(e);
                }
            }
        }
        if let Some(e) = last_err {
            // Channel is abandoned; next play reconnects.
            return Err(e);
        }

        let (stop_hb, hb) = LiveSession::start_heartbeat(Arc::clone(&channel));
        // Some Chromecast firmware revisions occasionally keep a freshly loaded
        // live stream in a silent pending state until an explicit media command.
        // A best-effort PLAY nudge mirrors the user workaround (pause/play) and
        // is harmless when autoplay already started successfully.
        if let Some(mid) = media_session_id {
            let _ = Self::nudge_play(&channel, &app.transport_id, mid);
        }
        *self.current.lock() = Some((
            device.discovered.id.clone(),
            LiveSession {
                channel,
                stop_hb,
                hb: Some(hb),
                transport_id: app.transport_id,
                session_id: app.session_id,
                media_session_id,
            },
        ));

        on_status(&format!("Playing on «{}»", device.discovered.name));
        log::info!("CastService::play Ok on '{}'", device.discovered.name);
        Ok(())
    }

    pub fn stop(&self) -> Result<(), CastError> {
        log::info!("CastService::stop");
        self.cancel.store(true, Ordering::SeqCst);
        let _op = self.op_lock.lock();
        let mut guard = self.current.lock();
        if let Some((_id, mut sess)) = guard.take() {
            sess.stop_heartbeat();
            let _ = Self::send_stop(&sess);
        }
        Ok(())
    }

    /// Volume for the current session.
    pub fn set_volume_current(&self, level: f32) -> Result<(), CastError> {
        let level = level.clamp(0.0, 1.0);
        let guard = self.current.lock();
        let Some((_id, sess)) = guard.as_ref() else {
            return Ok(());
        };
        let req = sess.channel.next_request_id();
        sess.channel.send_json(
            RECEIVER_ID,
            NS_RECEIVER,
            &json!({
                "type": "SET_VOLUME",
                "requestId": req,
                "volume": { "level": level }
            }),
        )?;
        Ok(())
    }

    fn take_or_connect(&self, device: &CastDeviceInfo) -> Result<Arc<CastChannel>, CastError> {
        if let Some((id, mut sess)) = self.current.lock().take()
            && id == device.discovered.id
        {
            sess.stop_heartbeat();
            return Ok(sess.channel);
        }
        Ok(Arc::new(CastChannel::connect(
            &device.discovered.host,
            device.discovered.port,
        )?))
    }

    fn send_stop(sess: &LiveSession) -> Result<(), CastError> {
        if let Some(media_id) = sess.media_session_id {
            let _ = sess.channel.send_json(
                &sess.transport_id,
                NS_MEDIA,
                &json!({
                    "type": "STOP",
                    "requestId": sess.channel.next_request_id(),
                    "mediaSessionId": media_id,
                }),
            );
        }
        let _ = sess.channel.send_json(
            RECEIVER_ID,
            NS_RECEIVER,
            &json!({
                "type": "STOP",
                "requestId": sess.channel.next_request_id(),
                "sessionId": sess.session_id,
            }),
        );
        let _ = sess.channel.send_json(
            &sess.transport_id,
            NS_CONNECTION,
            &json!({ "type": "CLOSE", "userAgent": "RockCast/0.1" }),
        );
        let _ = sess.channel.send_json(
            RECEIVER_ID,
            NS_CONNECTION,
            &json!({ "type": "CLOSE", "userAgent": "RockCast/0.1" }),
        );
        // Give the receiver time to finish reading STOP before tearing down TLS.
        thread::sleep(Duration::from_millis(150));
        Ok(())
    }

    fn nudge_play(
        channel: &CastChannel,
        transport_id: &str,
        media_session_id: i64,
    ) -> Result<(), CastError> {
        channel.send_json(
            transport_id,
            NS_MEDIA,
            &json!({
                "type": "PLAY",
                "requestId": channel.next_request_id(),
                "mediaSessionId": media_session_id,
            }),
        )?;
        Ok(())
    }

    fn load_media(
        channel: &CastChannel,
        app: &AppSession,
        url: &str,
        content_type: &str,
        title: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<i64>, CastError> {
        let req = channel.next_request_id();
        let mut idle_hits: u32 = 0;
        channel.send_json(
            &app.transport_id,
            NS_MEDIA,
            &json!({
                "type": "LOAD",
                "requestId": req,
                "sessionId": app.session_id,
                "autoplay": true,
                "currentTime": 0,
                "media": {
                    "contentId": url,
                    "streamType": "LIVE",
                    "contentType": content_type,
                    "metadata": {
                        "metadataType": 0,
                        "title": title
                    }
                },
                "customData": {}
            }),
        )?;

        channel
            .receive_find(cancel, Duration::from_secs(15), |msg| {
                if msg.namespace != NS_MEDIA {
                    return Ok(None);
                }
                let Payload::String(ref s) = msg.payload else {
                    return Ok(None);
                };
                let v: serde_json::Value =
                    serde_json::from_str(s).map_err(|e| ChannelError::Msg(e.to_string()))?;
                let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match typ {
                    "MEDIA_STATUS" => {
                        if let Some(st) = v
                            .get("status")
                            .and_then(|s| s.as_array())
                            .and_then(|a| a.first())
                        {
                            let content_ok = st
                                .pointer("/media/contentId")
                                .and_then(|c| c.as_str())
                                .is_some_and(|c| c == url);
                            let state = st
                                .get("playerState")
                                .and_then(|s| s.as_str())
                                .unwrap_or("");
                            if content_ok && state == "IDLE" {
                                idle_hits = idle_hits.saturating_add(1);
                                if idle_hits >= 10 {
                                    return Err(ChannelError::Msg(
                                        "Cast stayed IDLE too long after LOAD".into(),
                                    ));
                                }
                            } else if state == "BUFFERING" || state == "PLAYING" {
                                idle_hits = 0;
                            }
                        }
                        match classify_media_status(&v, req, url)? {
                            LoadProgress::Pending => Ok(None),
                            LoadProgress::Ready(mid) => Ok(Some(mid)),
                        }
                    }
                    "LOAD_FAILED" | "LOAD_CANCELLED" | "INVALID_REQUEST" | "ERROR" => {
                        let rid = v.get("requestId").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                        if rid == req || rid == 0 {
                            Err(ChannelError::Msg(format!("Cast rejected LOAD: {typ}")))
                        } else {
                            Ok(None)
                        }
                    }
                    _ => Ok(None),
                }
            })
            .map_err(Into::into)
    }

    fn ensure_media_receiver(&self, channel: &CastChannel) -> Result<AppSession, CastError> {
        if let Some(app) = self.query_dmr(channel)? {
            return Ok(app);
        }

        let req = channel.next_request_id();
        channel.send_json(
            RECEIVER_ID,
            NS_RECEIVER,
            &json!({
                "type": "LAUNCH",
                "requestId": req,
                "appId": DEFAULT_MEDIA_RECEIVER
            }),
        )?;

        channel
            .receive_find(&self.cancel, Duration::from_secs(12), |msg| {
                if msg.namespace != NS_RECEIVER {
                    return Ok(None);
                }
                let Payload::String(ref s) = msg.payload else {
                    return Ok(None);
                };
                let v: serde_json::Value =
                    serde_json::from_str(s).map_err(|e| ChannelError::Msg(e.to_string()))?;
                let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match typ {
                    "RECEIVER_STATUS" => {
                        if let Some(app) = parse_dmr(&v) {
                            return Ok(Some(app));
                        }
                        Ok(None)
                    }
                    "LAUNCH_ERROR" => {
                        let rid = v.get("requestId").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                        if rid == req {
                            Err(ChannelError::Msg(format!(
                                "LAUNCH_ERROR: {}",
                                v.get("reason").and_then(|r| r.as_str()).unwrap_or("?")
                            )))
                        } else {
                            Ok(None)
                        }
                    }
                    _ => Ok(None),
                }
            })
            .map_err(Into::into)
    }

    fn query_dmr(&self, channel: &CastChannel) -> Result<Option<AppSession>, CastError> {
        let req = channel.next_request_id();
        channel.send_json(
            RECEIVER_ID,
            NS_RECEIVER,
            &json!({ "type": "GET_STATUS", "requestId": req }),
        )?;

        let status = channel.receive_find(&self.cancel, Duration::from_secs(8), |msg| {
            if msg.namespace != NS_RECEIVER {
                return Ok(None);
            }
            let Payload::String(ref s) = msg.payload else {
                return Ok(None);
            };
            let v: serde_json::Value =
                serde_json::from_str(s).map_err(|e| ChannelError::Msg(e.to_string()))?;
            if v.get("type").and_then(|t| t.as_str()) != Some("RECEIVER_STATUS") {
                return Ok(None);
            }
            let rid = v.get("requestId").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
            if rid == req || rid == 0 {
                Ok(Some(v))
            } else {
                Ok(None)
            }
        })?;

        Ok(parse_dmr(&status))
    }
}

fn candidate_content_types(content_type: &str) -> Vec<String> {
    let mut out = Vec::new();
    let normalized = content_type.trim().to_ascii_lowercase();
    let mut push = |value: &str| {
        if !value.is_empty() && !out.iter().any(|existing| existing == value) {
            out.push(value.to_string());
        }
    };

    push(&normalized);
    if normalized.contains("opus") {
        push("audio/ogg; codecs=opus");
        push("audio/ogg");
        push("application/ogg");
        push("audio/opus");
    } else if normalized.contains("vorbis") || normalized == "audio/ogg" || normalized == "application/ogg" {
        push("audio/ogg; codecs=vorbis");
        push("audio/ogg");
        push("application/ogg");
    }
    if normalized != "audio/mpeg" {
        push("audio/mpeg");
    }
    out
}

fn classify_media_status(
    v: &serde_json::Value,
    req: u32,
    url: &str,
) -> Result<LoadProgress, ChannelError> {
    let rid = v.get("requestId").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    let status = v
        .get("status")
        .and_then(|s| s.as_array())
        .and_then(|a| a.first());
    let Some(st) = status else {
        return if rid == req {
            Ok(LoadProgress::Ready(None))
        } else {
            Ok(LoadProgress::Pending)
        };
    };

    let content_ok = st
        .pointer("/media/contentId")
        .and_then(|c| c.as_str())
        .is_some_and(|c| c == url);
    if rid != req && !content_ok {
        return Ok(LoadProgress::Pending);
    }

    let state = st
        .get("playerState")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let idle_reason = st
        .get("idleReason")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let mid = st.get("mediaSessionId").and_then(|m| m.as_i64());
    log::info!(
        "Cast LOAD status: state={} idleReason={} content_ok={} requestId={}",
        state,
        idle_reason,
        content_ok,
        rid
    );

    match state {
        "BUFFERING" | "PLAYING" | "PAUSED" => Ok(LoadProgress::Ready(mid)),
        "IDLE" if matches!(idle_reason, "ERROR" | "CANCELLED" | "INTERRUPTED" | "FINISHED") => {
            Err(ChannelError::Msg(format!(
                "Cast LOAD stalled in IDLE ({idle_reason})"
            )))
        }
        "IDLE" | "" => Ok(LoadProgress::Pending),
        _ => Ok(LoadProgress::Pending),
    }
}

#[derive(Clone)]
struct AppSession {
    transport_id: String,
    session_id: String,
}

fn parse_dmr(v: &serde_json::Value) -> Option<AppSession> {
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

#[cfg(test)]
mod tests {
    use super::{LoadProgress, candidate_content_types, classify_media_status};
    use serde_json::json;

    #[test]
    fn opus_candidates_include_ogg_variants() {
        let values = candidate_content_types("audio/ogg; codecs=opus");
        assert!(values.iter().any(|v| v == "audio/ogg; codecs=opus"));
        assert!(values.iter().any(|v| v == "audio/ogg"));
        assert!(values.iter().any(|v| v == "application/ogg"));
        assert!(values.iter().any(|v| v == "audio/opus"));
    }

    #[test]
    fn idle_status_is_not_treated_as_success() {
        let payload = json!({
            "type": "MEDIA_STATUS",
            "requestId": 7,
            "status": [{
                "mediaSessionId": 2,
                "playerState": "IDLE",
                "media": { "contentId": "http://127.0.0.1/stream" }
            }]
        });
        assert_eq!(
            classify_media_status(&payload, 7, "http://127.0.0.1/stream").unwrap(),
            LoadProgress::Pending
        );
    }

    #[test]
    fn buffering_status_is_treated_as_success() {
        let payload = json!({
            "type": "MEDIA_STATUS",
            "requestId": 7,
            "status": [{
                "mediaSessionId": 2,
                "playerState": "BUFFERING",
                "media": { "contentId": "http://127.0.0.1/stream" }
            }]
        });
        assert_eq!(
            classify_media_status(&payload, 7, "http://127.0.0.1/stream").unwrap(),
            LoadProgress::Ready(Some(2))
        );
    }
}
