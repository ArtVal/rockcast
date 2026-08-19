//! CASTV2 framing: TLS + 4-byte BE length + protobuf CastMessage.

mod auth;
mod consts;
mod error;
mod recv;
mod tls;

use std::{
    io::Write,
    time::Duration,
};

use parking_lot::Mutex as PMutex;

pub use consts::{
    DEFAULT_MEDIA_RECEIVER, NS_CONNECTION, NS_DEVICEAUTH, NS_HEARTBEAT, NS_MEDIA, NS_RECEIVER,
    RECEIVER_ID, SENDER_ID,
};
pub use error::ChannelError;
pub use tls::TlsStream;

use super::proto::{CastMessage, Payload};

pub struct CastChannel {
    pub(super) stream: PMutex<TlsStream>,
    pub(super) inbox: PMutex<Vec<CastMessage>>,
    pub(super) request_id: PMutex<u32>,
}

impl CastChannel {
    pub fn connect(host: &str, port: u16) -> Result<Self, ChannelError> {
        let stream = tls::connect(host, port)?;

        let channel = Self {
            stream: PMutex::new(stream),
            inbox: PMutex::new(Vec::new()),
            request_id: PMutex::new(1),
        };

        // Device auth (many receivers require a challenge after TLS).
        if let Err(err) = channel.authenticate() {
            log::warn!("device auth skipped/failed: {err}");
        }

        Ok(channel)
    }

    pub fn next_request_id(&self) -> u32 {
        let mut id = self.request_id.lock();
        let cur = *id;
        *id = id.wrapping_add(1).max(1);
        cur
    }

    pub fn send(&self, message: &CastMessage) -> Result<(), ChannelError> {
        let body = message.encode();
        let len = body.len() as u32;
        let mut header = [0u8; 4];
        header.copy_from_slice(&len.to_be_bytes());

        let mut stream = self.stream.lock();
        stream.write_all(&header)?;
        stream.write_all(&body)?;
        stream.flush()?;
        log::debug!(
            "cast → {} {} {:?}",
            message.namespace,
            message.destination_id,
            short_payload(message)
        );
        Ok(())
    }

    pub fn send_json(
        &self,
        destination: &str,
        namespace: &str,
        value: &serde_json::Value,
    ) -> Result<(), ChannelError> {
        let payload =
            serde_json::to_string(value).map_err(|e| ChannelError::Msg(format!("json: {e}")))?;
        self.send(&CastMessage::string(
            SENDER_ID,
            destination,
            namespace,
            payload,
        ))
    }

    pub fn set_read_timeout(&self, timeout: Duration) {
        let stream = self.stream.lock();
        let _ = stream.sock.set_read_timeout(Some(timeout));
    }

    pub fn ping(&self) -> Result<(), ChannelError> {
        self.send_json(
            RECEIVER_ID,
            NS_HEARTBEAT,
            &serde_json::json!({ "type": "PING" }),
        )
    }
}

pub(super) fn short_payload(msg: &CastMessage) -> String {
    match &msg.payload {
        Payload::String(s) => {
            if s.len() > 120 {
                format!("{}…", &s[..120])
            } else {
                s.clone()
            }
        }
        Payload::Binary(b) => format!("<{} bytes>", b.len()),
    }
}
