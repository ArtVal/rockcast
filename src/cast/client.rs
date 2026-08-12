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
    discovery::{DiscoveredDevice, DiscoveryError, discover},
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

        let mut content_types = vec![content_type.to_string()];
        if content_type != "audio/mpeg" {
            content_types.push("audio/mpeg".into());
        }

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

    fn load_media(
        channel: &CastChannel,
        app: &AppSession,
        url: &str,
        content_type: &str,
        title: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<i64>, CastError> {
        let req = channel.next_request_id();
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
                        let rid = v.get("requestId").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                        let status = v
                            .get("status")
                            .and_then(|s| s.as_array())
                            .and_then(|a| a.first());
                        if let Some(st) = status {
                            let content_ok = st
                                .pointer("/media/contentId")
                                .and_then(|c| c.as_str())
                                .is_some_and(|c| c == url);
                            if rid == req || content_ok {
                                let mid = st.get("mediaSessionId").and_then(|m| m.as_i64());
                                return Ok(Some(mid));
                            }
                        } else if rid == req {
                            return Ok(Some(None));
                        }
                        Ok(None)
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
