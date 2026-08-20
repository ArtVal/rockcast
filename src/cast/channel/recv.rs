//! Inbound message read loop, inbox, and heartbeat handling.

use std::{
    io::Read,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use super::super::proto::{CastMessage, Payload};
use super::{
    CastChannel, ChannelError,
    consts::{NS_HEARTBEAT, RECEIVER_ID},
};

impl CastChannel {
    /// Reads messages until `f` returns `Some`. Others go into the inbox
    /// (except heartbeat PING — we reply with PONG immediately).
    ///
    /// `cancel` aborts promptly (checked every ~read timeout). Wall-clock
    /// `overall` bounds the wait so a hung LOAD cannot block forever.
    pub fn receive_find<F, T>(
        &self,
        cancel: &AtomicBool,
        overall: Duration,
        mut f: F,
    ) -> Result<T, ChannelError>
    where
        F: FnMut(&CastMessage) -> Result<Option<T>, ChannelError>,
    {
        // Short reads so cancel/overall are noticed quickly even while the
        // receiver only sends heartbeats.
        self.set_read_timeout(Duration::from_millis(500));
        let result = self.receive_find_inner(cancel, overall, &mut f);
        self.set_read_timeout(Duration::from_millis(250));
        result
    }

    fn receive_find_inner<F, T>(
        &self,
        cancel: &AtomicBool,
        overall: Duration,
        f: &mut F,
    ) -> Result<T, ChannelError>
    where
        F: FnMut(&CastMessage) -> Result<Option<T>, ChannelError>,
    {
        let deadline = Instant::now() + overall;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(ChannelError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(ChannelError::Timeout);
            }

            // Re-scan inbox every iteration — heartbeat may have queued media.
            {
                let mut inbox = self.inbox.lock();
                let mut i = 0;
                while i < inbox.len() {
                    match f(&inbox[i])? {
                        Some(v) => {
                            inbox.remove(i);
                            return Ok(v);
                        }
                        None => i += 1,
                    }
                }
            }

            match self.read_one() {
                Ok(msg) => {
                    if self.handle_heartbeat(&msg)? {
                        continue;
                    }
                    if let Some(v) = f(&msg)? {
                        return Ok(v);
                    }
                    self.inbox.lock().push(msg);
                }
                Err(ChannelError::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn pump_heartbeats(&self) -> Result<(), ChannelError> {
        // Non-blocking: a short timeout is already set on the socket.
        match self.read_one() {
            Ok(msg) => {
                if !self.handle_heartbeat(&msg)? {
                    self.inbox.lock().push(msg);
                }
                Ok(())
            }
            Err(ChannelError::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn handle_heartbeat(&self, msg: &CastMessage) -> Result<bool, ChannelError> {
        if msg.namespace != NS_HEARTBEAT {
            return Ok(false);
        }
        let Payload::String(ref s) = msg.payload else {
            return Ok(true);
        };
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(s)
            && v.get("type").and_then(|t| t.as_str()) == Some("PING")
        {
            self.send_json(
                RECEIVER_ID,
                NS_HEARTBEAT,
                &serde_json::json!({ "type": "PONG" }),
            )?;
        }
        Ok(true)
    }

    fn read_one(&self) -> Result<CastMessage, ChannelError> {
        let mut stream = self.stream.lock();
        let mut header = [0u8; 4];
        stream.read_exact(&mut header)?;
        let len = u32::from_be_bytes(header);
        if len > 2 * 1024 * 1024 {
            return Err(ChannelError::Oversized(len));
        }
        let mut body = vec![0u8; len as usize];
        stream.read_exact(&mut body)?;
        let msg = CastMessage::decode(&body)?;
        log::debug!(
            "cast ← {} {} {:?}",
            msg.namespace,
            msg.source_id,
            super::short_payload(&msg)
        );
        Ok(msg)
    }
}
