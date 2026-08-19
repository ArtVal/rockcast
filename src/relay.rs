//! LAN HTTP relay: PC fetches a station stream (VPN-capable) and serves it to Cast.
//!
//! Design notes:
//! - Listener is non-blocking for accept; **accepted sockets are forced blocking**
//!   (on Windows they inherit non-blocking and write fails with WSAEWOULDBLOCK / 10035).
//! - One shared upstream feeder fills a ring; Cast clients read the live edge
//!   (avoids N× upstream when Cast opens multiple TCP connections).
//! - Spectrum/ICY taps still use the original station URL on the PC — EQ can move
//!   even when Cast audio has stalled.

use std::{
    collections::VecDeque,
    io::{Cursor, Read, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use parking_lot::{Condvar, Mutex};
use ropus::{Channels as OpusChannels, DecodeMode, Decoder as OpusDecoder};
use thiserror::Error;

use crate::audio::parse_stream_title;
use crate::net::{metadata_interval, stream_client, stream_headers};

const OPEN_TIMEOUT: Duration = Duration::from_secs(15);
const READ_POLL: Duration = Duration::from_millis(200);
const ACCEPT_POLL: Duration = Duration::from_millis(200);
const BUF: usize = 16 * 1024;
/// Keep ~1–2s of typical radio bitrate for late joiners / Cast dual-connect.
const RING_MAX: usize = 256 * 1024;
/// Join cushion when a client connects (don't start at empty live tip).
const JOIN_CUSHION: usize = 24 * 1024;

#[derive(Debug, Error)]
pub enum RelayError {
    #[error("no LAN IPv4 to advertise to Cast (check Wi‑Fi)")]
    NoLanIp,
    #[error("bind relay socket: {0}")]
    Bind(String),
    #[error("upstream open: {0}")]
    Upstream(String),
    #[error("relay cancelled")]
    Cancelled,
}

struct Session {
    stop: Arc<AtomicBool>,
    accept: Option<thread::JoinHandle<()>>,
    feeder: Option<thread::JoinHandle<()>>,
    public_url: String,
    tap_url: Option<String>,
    fanout: Arc<Fanout>,
}

#[derive(Clone)]
enum RelayTransport {
    Passthrough { content_type: String },
    WavPcm { sample_rate: u32, channels: u16 },
}

/// Shared live ring filled by one upstream reader.
struct Fanout {
    stop: Arc<AtomicBool>,
    inner: Mutex<FanoutInner>,
    cv: Condvar,
    /// Total bytes ever pushed (monotonic).
    written: AtomicU64,
    title: Mutex<Option<String>>,
}

struct FanoutInner {
    buf: VecDeque<u8>,
    /// Absolute byte index of `buf[0]`.
    start: u64,
    ended: bool,
    error: Option<String>,
}

impl Fanout {
    fn new(stop: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self {
            stop,
            inner: Mutex::new(FanoutInner {
                buf: VecDeque::with_capacity(RING_MAX),
                start: 0,
                ended: false,
                error: None,
            }),
            cv: Condvar::new(),
            written: AtomicU64::new(0),
            title: Mutex::new(None),
        })
    }

    fn push(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        {
            let mut g = self.inner.lock();
            g.buf.extend(data.iter().copied());
            while g.buf.len() > RING_MAX {
                g.buf.pop_front();
                g.start += 1;
            }
            self.written
                .store(g.start + g.buf.len() as u64, Ordering::SeqCst);
        }
        self.cv.notify_all();
    }

    fn set_title(&self, title: String) {
        let mut g = self.title.lock();
        if g.as_ref() != Some(&title) {
            *g = Some(title);
        }
    }

    fn take_title(&self) -> Option<String> {
        self.title.lock().clone()
    }

    fn finish_ok(&self) {
        self.inner.lock().ended = true;
        self.cv.notify_all();
    }

    fn finish_err(&self, msg: String) {
        {
            let mut g = self.inner.lock();
            g.ended = true;
            g.error = Some(msg);
        }
        self.cv.notify_all();
    }

    /// Copy available bytes at absolute `pos` into `out`. Advances `pos`.
    /// Returns Ok(0) on clean EOF, Err on feeder error / stop.
    fn read_at(&self, pos: &mut u64, out: &mut [u8]) -> Result<usize, String> {
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return Err("stopped".into());
            }
            let mut g = self.inner.lock();
            if let Some(err) = g.error.as_ref() {
                return Err(err.clone());
            }
            let end = g.start + g.buf.len() as u64;
            if *pos < g.start {
                // Client fell behind — jump to live edge minus a small cushion.
                let cushion = (JOIN_CUSHION as u64).min(g.buf.len() as u64 / 2);
                *pos = end.saturating_sub(cushion).max(g.start);
            }
            if *pos < end {
                let off = (*pos - g.start) as usize;
                let n = (g.buf.len() - off).min(out.len());
                for (i, b) in g.buf.iter().skip(off).take(n).enumerate() {
                    out[i] = *b;
                }
                *pos += n as u64;
                return Ok(n);
            }
            if g.ended {
                return Ok(0);
            }
            let _ = self.cv.wait_for(&mut g, Duration::from_millis(200));
        }
    }

    /// For MP3 consumers, joining from an arbitrary live-edge byte can land in
    /// the middle of a frame. Many decoders resync, but some Chromecast builds
    /// intermittently fail to start audio at all. Snap the initial position to
    /// the next likely MPEG frame header available in the current ring.
    fn align_mp3_pos(&self, pos: u64) -> u64 {
        let g = self.inner.lock();
        let end = g.start + g.buf.len() as u64;
        let pos = pos.clamp(g.start, end);
        let off = (pos - g.start) as usize;
        let bytes: Vec<u8> = g.buf.iter().skip(off).copied().collect();
        let Some(rel) = find_mpeg_frame_sync(&bytes) else {
            return pos;
        };
        pos + rel as u64
    }
}

/// Serves `GET /stream` by proxying the upstream radio URL, stripping ICY metadata.
pub struct StreamRelay {
    session: Mutex<Option<Session>>,
}

impl Default for StreamRelay {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamRelay {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
        }
    }

    pub fn stop(&self) {
        let mut guard = self.session.lock();
        if let Some(mut s) = guard.take() {
            log::info!("StreamRelay::stop url={}", s.public_url);
            s.stop.store(true, Ordering::SeqCst);
            if let Some(port) = port_from_url(&s.public_url) {
                let _ = TcpStream::connect_timeout(
                    &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
                    Duration::from_millis(200),
                );
            }
            // Never join network workers from the caller. A blocked upstream
            // socket is allowed to drain/exit after observing `stop`; retaining
            // the handles here previously froze UI play/stop and shutdown.
            drop(s.accept.take());
            drop(s.feeder.take());
        }
    }

    /// Start (or replace) a relay. `cast_host` is the Cast device IP/hostname for LAN advertise.
    /// Returns the URL Cast should LOAD (`http://<lan-ip>:<port>/stream`).
    pub fn start(
        &self,
        upstream_url: &str,
        cast_host: &str,
        preferred_content_type: &str,
        cancel: &AtomicBool,
    ) -> Result<(String, String), RelayError> {
        self.stop();
        if cancel.load(Ordering::SeqCst) {
            return Err(RelayError::Cancelled);
        }

        let advertise = advertise_ipv4_near(cast_host).ok_or(RelayError::NoLanIp)?;
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .map_err(|e| RelayError::Bind(e.to_string()))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| RelayError::Bind(e.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|e| RelayError::Bind(e.to_string()))?
            .port();
        let public_url = format!("http://{advertise}:{port}/stream");

        let transport = choose_transport(preferred_content_type, upstream_url);
        let (content_type, tap_url) = match &transport {
            RelayTransport::Passthrough { content_type } => (
                normalize_content_type(content_type),
                Some(tap_url_from_public(&public_url)),
            ),
            // Cast consumes a live WAV wrapper, while spectrum reuses the same
            // decoded PCM via /tap instead of reopening the upstream Opus URL.
            RelayTransport::WavPcm { .. } => {
                ("audio/wav".to_string(), Some(tap_url_from_public(&public_url)))
            }
        };
        if cancel.load(Ordering::SeqCst) {
            return Err(RelayError::Cancelled);
        }

        let stop = Arc::new(AtomicBool::new(false));
        let fanout = Fanout::new(Arc::clone(&stop));
        let upstream = upstream_url.to_string();
        let stop_feed = Arc::clone(&stop);
        let fan_feed = Arc::clone(&fanout);
        let feeder_transport = transport.clone();
        let feeder = thread::spawn(move || {
            let result = match feeder_transport {
                RelayTransport::Passthrough { .. } => run_feeder(&upstream, &fan_feed, &stop_feed),
                RelayTransport::WavPcm {
                    sample_rate,
                    channels,
                } => run_feeder_opus_wav(
                    &upstream,
                    &fan_feed,
                    &stop_feed,
                    sample_rate,
                    channels,
                ),
            };
            if let Err(e) = result {
                log::warn!("StreamRelay feeder end: {e}");
                fan_feed.finish_err(e);
            } else {
                fan_feed.finish_ok();
            }
        });

        // Do not block Cast LOAD on buffering — feeder fills while Cast connects.
        let stop_c = Arc::clone(&stop);
        let fan_c = Arc::clone(&fanout);
        let transport_serve = transport.clone();
        log::info!(
            "StreamRelay::start advertise={advertise}:{port} upstream={upstream_url} content-type={content_type}"
        );

        let accept = thread::spawn(move || {
            accept_loop(listener, fan_c, transport_serve, stop_c);
        });

        *self.session.lock() = Some(Session {
            stop,
            accept: Some(accept),
            feeder: Some(feeder),
            public_url: public_url.clone(),
            tap_url,
            fanout,
        });

        Ok((public_url, content_type))
    }

    pub fn public_url(&self) -> Option<String> {
        self.session.lock().as_ref().map(|s| s.public_url.clone())
    }

    /// URL for internal metadata/spectrum tap (raw HTTP body, no chunk framing).
    pub fn tap_url(&self) -> Option<String> {
        self.session.lock().as_ref().and_then(|s| s.tap_url.clone())
    }

    /// Latest ICY StreamTitle from the upstream feeder (relay mode).
    pub fn latest_title(&self) -> Option<String> {
        self.session
            .lock()
            .as_ref()
            .and_then(|s| s.fanout.take_title())
    }

    /// Best-effort warmup for Cast startup: wait until feeder accumulated
    /// at least `min_bytes` in the shared ring, or until `timeout`.
    pub fn wait_for_data(&self, min_bytes: usize, timeout: Duration) -> bool {
        if min_bytes == 0 {
            return true;
        }
        let Some(session) = self.session.lock().as_ref().map(|s| Arc::clone(&s.fanout)) else {
            return false;
        };
        let deadline = Instant::now() + timeout;
        loop {
            let written = session.written.load(Ordering::Acquire) as usize;
            if written >= min_bytes {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

fn port_from_url(url: &str) -> Option<u16> {
    let rest = url.strip_prefix("http://")?;
    let hostport = rest.split('/').next()?;
    let port = hostport.rsplit_once(':')?.1;
    port.parse().ok()
}

fn accept_loop(
    listener: TcpListener,
    fanout: Arc<Fanout>,
    transport: RelayTransport,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if stop.load(Ordering::SeqCst) {
                    let _ = stream.shutdown(Shutdown::Both);
                    break;
                }
                // Critical on Windows: accepted sockets inherit non-blocking from the listener.
                if let Err(e) = stream.set_nonblocking(false) {
                    log::warn!("StreamRelay: set_nonblocking(false) failed for {peer}: {e}");
                }
                let _ = stream.set_nodelay(true);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
                log::info!("StreamRelay: client {peer}");
                let fan = Arc::clone(&fanout);
                let transport = transport.clone();
                let stop_c = Arc::clone(&stop);
                thread::spawn(move || {
                    if let Err(e) = handle_client(stream, &fan, &transport, &stop_c) {
                        log::warn!("StreamRelay client {peer} end: {e}");
                    } else {
                        log::info!("StreamRelay client {peer} closed cleanly");
                    }
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(e) => {
                log::warn!("StreamRelay accept: {e}");
                thread::sleep(ACCEPT_POLL);
            }
        }
    }
    log::info!("StreamRelay accept loop exit");
}

fn handle_client(
    mut stream: TcpStream,
    fanout: &Fanout,
    transport: &RelayTransport,
    stop: &AtomicBool,
) -> Result<(), String> {
    let mut req = Vec::with_capacity(1024);
    let mut tmp = [0u8; 512];
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err("stopped".into());
        }
        if Instant::now() > deadline {
            return Err("request header timeout".into());
        }
        match stream.read(&mut tmp) {
            Ok(0) => return Err("client closed before request".into()),
            Ok(n) => {
                req.extend_from_slice(&tmp[..n]);
                if req.windows(4).any(|w| w == b"\r\n\r\n") || req.len() > 16 * 1024 {
                    break;
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(e) => return Err(e.to_string()),
        }
    }

    let head = String::from_utf8_lossy(&req);
    let first = head.lines().next().unwrap_or("");
    let is_get = first.starts_with("GET ") || first.starts_with("HEAD ");
    let is_head = first.starts_with("HEAD ");
    let path = first.split_whitespace().nth(1).unwrap_or("/");
    let is_tap = path == "/tap";
    let is_stream = path == "/stream" || path == "/";
    let tap_allowed = true;
    if !is_get || (!is_stream && !(is_tap && tap_allowed)) {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n");
        return Err(format!("bad request: {first}"));
    }

    let headers = if is_tap {
        match transport {
            RelayTransport::Passthrough { content_type } => tap_response_headers(content_type),
            RelayTransport::WavPcm {
                sample_rate,
                channels,
            } => pcm_tap_response_headers(*sample_rate, *channels),
        }
    } else {
        match transport {
            RelayTransport::Passthrough { content_type } => stream_response_headers(content_type),
            RelayTransport::WavPcm {
                sample_rate,
                channels,
            } => wav_response_headers(*sample_rate, *channels),
        }
    };
    write_all_retry(&mut stream, headers.as_bytes(), stop)?;
    if is_head {
        return Ok(());
    }
    if !is_tap
        && let RelayTransport::WavPcm {
            sample_rate,
            channels,
        } = transport
    {
        let header = wav_live_header(*sample_rate, *channels);
        write_all_retry(&mut stream, &header, stop)?;
    }

    // Join near the live edge (with cushion if buffer is warm).
    let mut pos = {
        let g = fanout.inner.lock();
        let end = g.start + g.buf.len() as u64;
        let cushion = (JOIN_CUSHION as u64).min(g.buf.len() as u64);
        end.saturating_sub(cushion).max(g.start)
    };
    let content_type = match transport {
        RelayTransport::Passthrough { content_type } => content_type.as_str(),
        RelayTransport::WavPcm { .. } => "audio/wav",
    };
    if content_type.starts_with("audio/mpeg") || content_type.starts_with("audio/mp3") {
        pos = fanout.align_mp3_pos(pos);
    }

    let mut buf = vec![0u8; BUF];
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err("stopped".into());
        }
        let n = fanout.read_at(&mut pos, &mut buf)?;
        if n == 0 {
            if !is_tap && matches!(transport, RelayTransport::Passthrough { .. }) {
                write_all_retry(&mut stream, b"0\r\n\r\n", stop)?;
            }
            return Ok(());
        }
        if is_tap || matches!(transport, RelayTransport::WavPcm { .. }) {
            write_all_retry(&mut stream, &buf[..n], stop)?;
        } else {
            write_chunk(&mut stream, &buf[..n], stop)?;
        }
    }
}

/// HTTP/1.1 framing for an indefinite audio stream.
///
/// Chromecast's Default Media Receiver does not reliably consume an HTTP/1.1
/// close-delimited response as a live stream. Chunked transfer encoding makes
/// every completed relay read immediately visible without inventing a length.
fn stream_response_headers(content_type: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {content_type}\r\n\
         Cache-Control: no-cache, no-store\r\n\
         Pragma: no-cache\r\n\
         Connection: keep-alive\r\n\
         Transfer-Encoding: chunked\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Accept-Ranges: none\r\n\
         \r\n"
    )
}

fn tap_response_headers(content_type: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {content_type}\r\n\
         Cache-Control: no-cache, no-store\r\n\
         Pragma: no-cache\r\n\
         Connection: keep-alive\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Accept-Ranges: none\r\n\
         \r\n"
    )
}

fn pcm_tap_response_headers(sample_rate: u32, channels: u16) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: audio/L16\r\n\
         Cache-Control: no-cache, no-store\r\n\
         Pragma: no-cache\r\n\
         Connection: keep-alive\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Accept-Ranges: none\r\n\
         X-Audio-Sample-Rate: {sample_rate}\r\n\
         X-Audio-Channels: {channels}\r\n\
         \r\n"
    )
}

fn write_chunk(stream: &mut TcpStream, data: &[u8], stop: &AtomicBool) -> Result<(), String> {
    debug_assert!(!data.is_empty());
    let prefix = format!("{:X}\r\n", data.len());
    write_all_retry(stream, prefix.as_bytes(), stop)?;
    write_all_retry(stream, data, stop)?;
    write_all_retry(stream, b"\r\n", stop)
}

fn write_all_retry(
    stream: &mut TcpStream,
    mut data: &[u8],
    stop: &AtomicBool,
) -> Result<(), String> {
    while !data.is_empty() {
        if stop.load(Ordering::SeqCst) {
            return Err("stopped".into());
        }
        match stream.write(data) {
            Ok(0) => return Err("client write closed".into()),
            Ok(n) => data = &data[n..],
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(format!("client write: {e}")),
        }
    }
    Ok(())
}

fn find_mpeg_frame_sync(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|w| {
        let b0 = w[0];
        let b1 = w[1];
        b0 == 0xFF && (b1 & 0xE0) == 0xE0 && (b1 & 0x18) != 0x08
    })
}

fn run_feeder(url: &str, fanout: &Fanout, stop: &AtomicBool) -> Result<(), String> {
    let headers = stream_headers(false);
    let client = stream_client(Duration::from_secs(10), None)?;

    let resp = open_upstream(client, url, headers, stop)?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let meta_int = metadata_interval(resp.headers());
    log::info!(
        "StreamRelay feeder HTTP ok content-type={} icy-metaint={}",
        resp.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("?"),
        meta_int
    );

    let mut body = resp;
    let mut buf = vec![0u8; BUF];
    let mut until_meta = meta_int;
    let mut meta_left: Option<usize> = None;
    let mut meta_buf = Vec::new();
    let mut audio = Vec::with_capacity(BUF);
    let mut last_title = String::new();

    loop {
        if stop.load(Ordering::SeqCst) {
            return Err("stopped".into());
        }
        let n = match body.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        };

        if meta_int == 0 {
            fanout.push(&buf[..n]);
            continue;
        }

        audio.clear();
        let mut i = 0;
        while i < n {
            if stop.load(Ordering::SeqCst) {
                return Err("stopped".into());
            }
            if let Some(left) = meta_left.as_mut() {
                let take = (*left).min(n - i);
                meta_buf.extend_from_slice(&buf[i..i + take]);
                i += take;
                *left -= take;
                if *left == 0 {
                    if let Some(title) = parse_stream_title(&meta_buf) {
                        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
                        if !title.is_empty() && title != last_title {
                            last_title = title.clone();
                            fanout.set_title(title);
                        }
                    }
                    meta_buf.clear();
                    meta_left = None;
                    until_meta = meta_int;
                }
                continue;
            }
            let take = until_meta.min(n - i);
            audio.extend_from_slice(&buf[i..i + take]);
            i += take;
            until_meta -= take;
            if until_meta == 0 {
                let len_byte = if i < n {
                    let b = buf[i];
                    i += 1;
                    b
                } else {
                    let mut len_buf = [0u8; 1];
                    body.read_exact(&mut len_buf).map_err(|e| e.to_string())?;
                    len_buf[0]
                };
                let meta_len = (len_byte as usize) * 16;
                if meta_len == 0 {
                    until_meta = meta_int;
                } else {
                    meta_buf.clear();
                    meta_left = Some(meta_len);
                }
            }
        }
        if !audio.is_empty() {
            fanout.push(&audio);
        }
    }
}

fn run_feeder_opus_wav(
    url: &str,
    fanout: &Fanout,
    stop: &AtomicBool,
    sample_rate: u32,
    channels: u16,
) -> Result<(), String> {
    let headers = stream_headers(false);
    let client = stream_client(Duration::from_secs(10), None)?;
    let resp = open_upstream(client, url, headers, stop)?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    log::info!(
        "StreamRelay feeder HTTP ok content-type={} icy-metaint={}",
        resp.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("?"),
        metadata_interval(resp.headers())
    );

    let mut reader = LiveOggOpusReader::new(resp);
    let mut decoder = OpusDecoder::new(sample_rate, ropus_channels(channels)?)
        .map_err(|e| format!("opus decoder: {e}"))?;
    let mut pcm = vec![0i16; 5760 * usize::from(channels)];
    while !stop.load(Ordering::SeqCst) {
        let packet = match reader.read_packet(stop)? {
            Some(packet) => packet,
            None => return Ok(()),
        };
        let samples = match decoder.decode(&packet, &mut pcm, DecodeMode::Normal) {
            Ok(samples) => samples,
            Err(e) => {
                log::warn!("opus decode packet error: {e}");
                continue;
            }
        };
        if samples == 0 {
            continue;
        }
        let frames = samples * usize::from(channels);
        let bytes = pcm[..frames]
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<u8>>();
        fanout.push(&bytes);
    }
    Err("stopped".into())
}

fn ropus_channels(channels: u16) -> Result<OpusChannels, String> {
    match channels {
        1 => Ok(OpusChannels::Mono),
        2 => Ok(OpusChannels::Stereo),
        other => Err(format!("unsupported opus channel count: {other}")),
    }
}

struct LiveOggOpusReader<R: Read> {
    inner: R,
    packet: Vec<u8>,
    segments: VecDeque<Vec<u8>>,
    continued_at_page_start: Option<bool>,
    skipping_continued: bool,
    saw_head: bool,
    saw_tags: bool,
}

impl<R: Read> LiveOggOpusReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            packet: Vec::new(),
            segments: VecDeque::new(),
            continued_at_page_start: None,
            skipping_continued: false,
            saw_head: false,
            saw_tags: false,
        }
    }

    fn read_packet(&mut self, stop: &AtomicBool) -> Result<Option<Vec<u8>>, String> {
        loop {
            if self.segments.is_empty() {
                let page = match self.read_page(stop)? {
                    Some(page) => page,
                    None => return Ok(None),
                };
                self.continued_at_page_start = Some(page.continued);
                self.segments = page.segments.into();
            }
            if self.continued_at_page_start.take().unwrap_or(false) && self.packet.is_empty() {
                self.skipping_continued = true;
            }
            while let Some(segment) = self.segments.pop_front() {
                if self.skipping_continued {
                    if segment.len() < 255 {
                        self.skipping_continued = false;
                    }
                    continue;
                }
                self.packet.extend_from_slice(&segment);
                if segment.len() < 255 {
                    let packet = std::mem::take(&mut self.packet);
                    if !self.saw_head {
                        if !packet.starts_with(b"OpusHead") {
                            return Err("ogg/opus stream missing OpusHead".into());
                        }
                        self.saw_head = true;
                        continue;
                    }
                    if !self.saw_tags {
                        self.saw_tags = true;
                        continue;
                    }
                    return Ok(Some(packet));
                }
            }
        }
    }

    fn read_page(&mut self, stop: &AtomicBool) -> Result<Option<OggPage>, String> {
        let mut header = [0u8; 27];
        if !read_exact_or_eof(&mut self.inner, &mut header, stop)? {
            return Ok(None);
        }
        if &header[0..4] != b"OggS" {
            return Err("invalid Ogg capture pattern".into());
        }
        let continued = (header[5] & 0x01) != 0;
        let segments_len = header[26] as usize;
        let mut lacing = vec![0u8; segments_len];
        read_exact_checked(&mut self.inner, &mut lacing, stop)?;
        let payload_len: usize = lacing.iter().map(|&v| usize::from(v)).sum();
        let mut payload = vec![0u8; payload_len];
        read_exact_checked(&mut self.inner, &mut payload, stop)?;
        let mut cursor = Cursor::new(payload);
        let mut segments = Vec::with_capacity(segments_len);
        for &len in &lacing {
            let mut part = vec![0u8; usize::from(len)];
            cursor
                .read_exact(&mut part)
                .map_err(|e| format!("ogg payload: {e}"))?;
            segments.push(part);
        }
        Ok(Some(OggPage {
            continued,
            segments,
        }))
    }
}

struct OggPage {
    continued: bool,
    segments: Vec<Vec<u8>>,
}

fn read_exact_or_eof<R: Read>(
    reader: &mut R,
    buf: &mut [u8],
    stop: &AtomicBool,
) -> Result<bool, String> {
    let mut filled = 0;
    while filled < buf.len() {
        if stop.load(Ordering::SeqCst) {
            return Err("stopped".into());
        }
        match reader.read(&mut buf[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err("unexpected eof".into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(true)
}

fn read_exact_checked<R: Read>(
    reader: &mut R,
    buf: &mut [u8],
    stop: &AtomicBool,
) -> Result<(), String> {
    if read_exact_or_eof(reader, buf, stop)? {
        Ok(())
    } else {
        Err("unexpected eof".into())
    }
}

fn open_upstream(
    client: reqwest::blocking::Client,
    url: &str,
    headers: reqwest::header::HeaderMap,
    stop: &AtomicBool,
) -> Result<reqwest::blocking::Response, String> {
    let url = url.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let result = client.get(url).headers(headers).send();
        let _ = tx.send(result);
    });

    let deadline = Instant::now() + OPEN_TIMEOUT;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err("stopped".into());
        }
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(READ_POLL);
        if wait.is_zero() {
            return Err("upstream open timeout".into());
        }
        match rx.recv_timeout(wait) {
            Ok(Ok(resp)) => return Ok(resp),
            Ok(Err(e)) => return Err(e.to_string()),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("failed to open upstream".into());
            }
        }
    }
}

fn normalize_content_type(ct: &str) -> String {
    let normalized = ct.trim();
    let base = normalized.split(';').next().unwrap_or(normalized).trim();
    if base.is_empty() || base.eq_ignore_ascii_case("application/octet-stream") {
        return "audio/mpeg".into();
    }
    normalized.to_string()
}

pub fn tap_url_from_public(url: &str) -> String {
    url.replacen("/stream", "/tap", 1)
}

fn choose_transport(preferred_content_type: &str, upstream_url: &str) -> RelayTransport {
    if should_transcode_to_wav(preferred_content_type, upstream_url) {
        RelayTransport::WavPcm {
            sample_rate: 48_000,
            channels: 2,
        }
    } else {
        RelayTransport::Passthrough {
            content_type: preferred_content_type.to_string(),
        }
    }
}

fn should_transcode_to_wav(content_type: &str, url: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    let url = url.to_ascii_lowercase();
    ct.contains("opus") || url.ends_with(".opus") || url.contains(".opus?")
}

fn wav_response_headers(sample_rate: u32, channels: u16) -> String {
    let bits_per_sample = 16u16;
    let block_align = channels.saturating_mul(bits_per_sample / 8);
    let byte_rate = sample_rate.saturating_mul(u32::from(block_align));
    let content_length = 44u64 + u64::from(u32::MAX);
    format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: audio/wav\r\n\
         Content-Length: {content_length}\r\n\
         Cache-Control: no-cache, no-store\r\n\
         Pragma: no-cache\r\n\
         Connection: keep-alive\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Accept-Ranges: none\r\n\
         X-Audio-Sample-Rate: {sample_rate}\r\n\
         X-Audio-Channels: {channels}\r\n\
         X-Audio-Byte-Rate: {byte_rate}\r\n\
         \r\n"
    )
}

fn wav_live_header(sample_rate: u32, channels: u16) -> [u8; 44] {
    let bits_per_sample = 16u16;
    let block_align = channels * (bits_per_sample / 8);
    let byte_rate = sample_rate * u32::from(block_align);
    let riff_size = u32::MAX;
    let data_size = u32::MAX;
    let mut out = [0u8; 44];
    out[0..4].copy_from_slice(b"RIFF");
    out[4..8].copy_from_slice(&riff_size.to_le_bytes());
    out[8..12].copy_from_slice(b"WAVE");
    out[12..16].copy_from_slice(b"fmt ");
    out[16..20].copy_from_slice(&16u32.to_le_bytes());
    out[20..22].copy_from_slice(&1u16.to_le_bytes());
    out[22..24].copy_from_slice(&channels.to_le_bytes());
    out[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    out[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    out[32..34].copy_from_slice(&block_align.to_le_bytes());
    out[34..36].copy_from_slice(&bits_per_sample.to_le_bytes());
    out[36..40].copy_from_slice(b"data");
    out[40..44].copy_from_slice(&data_size.to_le_bytes());
    out
}

fn advertise_ipv4_near(cast_host: &str) -> Option<Ipv4Addr> {
    if let Ok(ip) = std::env::var("ROCKCAST_RELAY_ADVERTISE_IP")
        && let Ok(ip) = ip.parse::<Ipv4Addr>()
    {
        return Some(ip);
    }
    let peer = resolve_ipv4(cast_host)?;
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return None;
    };

    let mut same_net: Option<(i32, Ipv4Addr)> = None;
    let mut best_lan: Option<(i32, Ipv4Addr)> = None;

    for iface in ifaces {
        if iface.is_loopback() || is_vpn_or_virtual(&iface.name) {
            continue;
        }
        let if_addrs::IfAddr::V4(ref v4) = iface.addr else {
            continue;
        };
        let ip = v4.ip;
        if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
            continue;
        }
        if !ip.is_private() && !is_apipa(ip) {
            continue;
        }
        let score = score_lan(&iface.name, ip, is_apipa(ip), iface.is_oper_up());
        let netmask = v4.netmask;
        if in_same_subnet(ip, netmask, peer) && same_net.map(|(s, _)| score > s).unwrap_or(true) {
            same_net = Some((score, ip));
        }
        if best_lan.map(|(s, _)| score > s).unwrap_or(true) {
            best_lan = Some((score, ip));
        }
    }

    same_net.or(best_lan).map(|(_, ip)| ip)
}

fn resolve_ipv4(host: &str) -> Option<Ipv4Addr> {
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return Some(ip);
    }
    use std::net::ToSocketAddrs;
    let addrs = format!("{host}:0").to_socket_addrs().ok()?;
    for a in addrs {
        if let SocketAddr::V4(v4) = a {
            return Some(*v4.ip());
        }
    }
    None
}

fn in_same_subnet(ip: Ipv4Addr, mask: Ipv4Addr, peer: Ipv4Addr) -> bool {
    let ip_u = u32::from(ip);
    let mask_u = u32::from(mask);
    let peer_u = u32::from(peer);
    (ip_u & mask_u) == (peer_u & mask_u)
}

fn is_apipa(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 169 && o[1] == 254
}

fn score_lan(name: &str, ip: Ipv4Addr, apipa: bool, oper_up: bool) -> i32 {
    let lower = name.to_lowercase();
    let mut score = 0;
    if lower.contains("wi-fi")
        || lower.contains("wifi")
        || lower.contains("wlan")
        || lower.contains("wireless")
        || lower.contains("беспровод")
    {
        score += 100;
    }
    if lower.contains("ethernet") || lower.contains("локальн") {
        score += 80;
    } else if lower.contains("eth") || lower.ends_with(" lan") || lower.contains(" lan ") {
        score += 60;
    }
    let o = ip.octets();
    if o[0] == 192 && o[1] == 168 {
        score += 50;
    } else if o[0] == 10 {
        score += 30;
    } else if o[0] == 172 && (16..=31).contains(&o[1]) {
        score += 10;
    }
    if apipa {
        score -= 40;
    }
    if oper_up {
        score += 20;
    }
    score
}

fn is_vpn_or_virtual(name: &str) -> bool {
    let lower = name.to_lowercase();
    const MARKERS: &[&str] = &[
        "amnezia",
        "wintun",
        "wireguard",
        "outline",
        "nordlynx",
        "nordvpn",
        "openvpn",
        "tap-windows",
        "tap-win",
        "tunnel",
        "vpn",
        "utun",
        "warp",
        "cloudflare",
        "zerotier",
        "tailscale",
        "hamachi",
        "radmin",
        "softether",
        "vethernet",
        "hyper-v",
        "virtualbox",
        "vmware",
        "vmnet",
        "wsl",
        "docker",
        "bravetunnel",
        "npcap",
        "loopback",
        "bluetooth",
        "isatap",
        "teredo",
        "microsoft wi-fi direct",
    ];
    if MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    lower.starts_with("tap")
        || lower.starts_with("tun")
        || lower == "wg"
        || lower.starts_with("wg-")
}

#[cfg(test)]
mod tests {
    use super::{
        choose_transport, normalize_content_type, stream_response_headers, tap_url_from_public,
        wav_live_header, RelayTransport,
    };

    #[test]
    fn live_stream_response_uses_chunked_encoding() {
        let headers = stream_response_headers("audio/mpeg");
        assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(headers.contains("Content-Type: audio/mpeg\r\n"));
        assert!(headers.contains("Transfer-Encoding: chunked\r\n"));
        assert!(!headers.contains("Content-Length:"));
        assert!(headers.ends_with("\r\n\r\n"));
    }

    #[test]
    fn normalize_content_type_preserves_codec_parameters() {
        assert_eq!(
            normalize_content_type("audio/ogg; codecs=opus"),
            "audio/ogg; codecs=opus"
        );
    }

    #[test]
    fn tap_url_uses_raw_tap_endpoint() {
        assert_eq!(
            tap_url_from_public("http://192.168.31.133:56356/stream"),
            "http://192.168.31.133:56356/tap"
        );
    }

    #[test]
    fn opus_transport_uses_pcm_tap_endpoint() {
        let public_url = "http://192.168.31.133:56356/stream".to_string();
        let (_content_type, tap_url) = match (RelayTransport::WavPcm {
            sample_rate: 48_000,
            channels: 2,
        }) {
            RelayTransport::Passthrough { content_type } => (
                normalize_content_type(&content_type),
                Some(tap_url_from_public(&public_url)),
            ),
            RelayTransport::WavPcm { .. } => {
                ("audio/wav".to_string(), Some(tap_url_from_public(&public_url)))
            }
        };
        assert_eq!(tap_url.as_deref(), Some("http://192.168.31.133:56356/tap"));
    }

    #[test]
    fn opus_stream_uses_wav_transport() {
        match choose_transport("audio/ogg; codecs=opus", "http://x/stream.opus") {
            RelayTransport::WavPcm {
                sample_rate,
                channels,
            } => {
                assert_eq!(sample_rate, 48_000);
                assert_eq!(channels, 2);
            }
            RelayTransport::Passthrough { .. } => panic!("expected wav transcode"),
        }
    }

    #[test]
    fn wav_header_looks_valid() {
        let header = wav_live_header(48_000, 2);
        assert_eq!(&header[0..4], b"RIFF");
        assert_eq!(&header[8..12], b"WAVE");
        assert_eq!(&header[12..16], b"fmt ");
        assert_eq!(&header[36..40], b"data");
    }
}
