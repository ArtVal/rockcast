//! CASTV2 framing: TLS + 4-byte BE length + protobuf CastMessage.

use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, StreamOwned,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use thiserror::Error;

pub use super::proto::Payload;
use super::proto::{CastMessage, ProtoError, encode_auth_challenge};

pub const SENDER_ID: &str = "sender-0";
pub const RECEIVER_ID: &str = "receiver-0";
pub const NS_CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
pub const NS_HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
pub const NS_RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
pub const NS_MEDIA: &str = "urn:x-cast:com.google.cast.media";
pub const NS_DEVICEAUTH: &str = "urn:x-cast:com.google.cast.tp.deviceauth";
pub const DEFAULT_MEDIA_RECEIVER: &str = "CC1AD845";

#[derive(Debug, Error)]
pub enum ChannelError {
    #[error("TCP: {0}")]
    Io(#[from] std::io::Error),
    #[error("TLS: {0}")]
    Tls(String),
    #[error("protobuf: {0}")]
    Proto(#[from] ProtoError),
    #[error("message too large ({0} bytes)")]
    Oversized(u32),
    #[error("timed out waiting for Cast response")]
    Timeout,
    #[error("Cast operation cancelled")]
    Cancelled,
    #[error("{0}")]
    Msg(String),
}

pub type TlsStream = StreamOwned<ClientConnection, TcpStream>;

pub struct CastChannel {
    stream: Mutex<TlsStream>,
    inbox: Mutex<Vec<CastMessage>>,
    request_id: Mutex<u32>,
}

impl CastChannel {
    pub fn connect(host: &str, port: u16) -> Result<Self, ChannelError> {
        let addr = format!("{host}:{port}");
        let tcp = TcpStream::connect(addr)?;
        tcp.set_nodelay(true)?;
        // Short default timeout: heartbeat must not block SET_VOLUME for seconds.
        tcp.set_read_timeout(Some(Duration::from_millis(250)))?;
        tcp.set_write_timeout(Some(Duration::from_secs(5)))?;

        let config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertVerification))
            .with_no_client_auth();

        let server_name = ServerName::try_from(host.to_string())
            .map_err(|e| ChannelError::Tls(format!("ServerName: {e}")))?;
        let conn = ClientConnection::new(Arc::new(config), server_name)
            .map_err(|e| ChannelError::Tls(e.to_string()))?;
        let stream = StreamOwned::new(conn, tcp);

        let channel = Self {
            stream: Mutex::new(stream),
            inbox: Mutex::new(Vec::new()),
            request_id: Mutex::new(1),
        };

        // Device auth (many receivers require a challenge after TLS).
        if let Err(err) = channel.authenticate() {
            log::warn!("device auth skipped/failed: {err}");
        }

        Ok(channel)
    }

    fn authenticate(&self) -> Result<(), ChannelError> {
        let challenge = CastMessage::binary(
            SENDER_ID,
            RECEIVER_ID,
            NS_DEVICEAUTH,
            encode_auth_challenge(),
        );
        self.send(&challenge)?;
        // Wait for the auth reply; anything else goes into the inbox.
        let never = AtomicBool::new(false);
        let _ = self.receive_find(&never, Duration::from_secs(8), |msg| {
            if msg.namespace == NS_DEVICEAUTH {
                Ok(Some(()))
            } else {
                Ok(None)
            }
        });
        Ok(())
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

    pub fn ping(&self) -> Result<(), ChannelError> {
        self.send_json(
            RECEIVER_ID,
            NS_HEARTBEAT,
            &serde_json::json!({ "type": "PING" }),
        )
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
            short_payload(&msg)
        );
        Ok(msg)
    }
}

fn short_payload(msg: &CastMessage) -> String {
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

#[derive(Debug)]
struct NoCertVerification;

impl ServerCertVerifier for NoCertVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
