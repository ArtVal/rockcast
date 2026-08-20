//! High-level CASTV2: CONNECT → LAUNCH DMR → LOAD live stream.

mod error;
mod media;
mod session;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use parking_lot::Mutex;
use serde_json::json;

pub use error::CastError;
use media::{candidate_content_types, classify_media_status};
use session::{AppSession, LiveSession, LoadProgress, parse_dmr};

use super::{
    channel::{
        CastChannel, ChannelError, DEFAULT_MEDIA_RECEIVER, NS_CONNECTION, NS_MEDIA, NS_RECEIVER,
        RECEIVER_ID,
    },
    discovery::{DiscoveredDevice, discover, discover_streaming},
    proto::Payload,
};

#[derive(Clone)]
pub struct CastDeviceInfo {
    pub discovered: DiscoveredDevice,
}

impl CastDeviceInfo {
    pub fn label(&self) -> String {
        self.discovered.label()
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
                            let state =
                                st.get("playerState").and_then(|s| s.as_str()).unwrap_or("");
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
