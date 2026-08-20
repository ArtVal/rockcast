//! LAN HTTP relay: PC fetches a station stream and serves it to Cast.

mod error;
mod fanout;
mod feeder;
mod net;
mod server;
mod transport;
mod wav;

use std::{
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use parking_lot::Mutex;

pub use error::RelayError;
pub use fanout::Fanout;
pub use transport::{
    RelayTransport, TranscodeKind, choose_transport, normalize_content_type, tap_url_from_public,
};

use feeder::{run_feeder_passthrough, run_feeder_transcode};
use net::advertise_ipv4_near;
use server::accept_loop;

struct Session {
    stop: Arc<AtomicBool>,
    accept: Option<thread::JoinHandle<()>>,
    feeder: Option<thread::JoinHandle<()>>,
    public_url: String,
    tap_url: Option<String>,
    fanout: Arc<Fanout>,
}

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
        let ended = {
            let mut guard = self.session.lock();
            guard.take()
        };
        if let Some(mut s) = ended {
            log::info!("StreamRelay::stop url={}", s.public_url);
            s.stop.store(true, Ordering::SeqCst);
            if let Some(port) = port_from_url(&s.public_url) {
                let _ = TcpStream::connect_timeout(
                    &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
                    Duration::from_millis(200),
                );
            }
            detach_relay_worker(s.accept.take(), "accept");
            detach_relay_worker(s.feeder.take(), "feeder");
        }
    }

    pub fn is_active(&self) -> bool {
        self.session.lock().is_some()
    }

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
            RelayTransport::WavPcm { .. } => (
                "audio/wav".to_string(),
                Some(tap_url_from_public(&public_url)),
            ),
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
            let _worker = crate::profile::worker("relay_feeder");
            loop {
                if stop_feed.load(Ordering::SeqCst) {
                    break;
                }
                let started = Instant::now();
                let result = match &feeder_transport {
                    RelayTransport::Passthrough { .. } => {
                        run_feeder_passthrough(&upstream, &fan_feed, &stop_feed)
                    }
                    RelayTransport::WavPcm { .. } => run_feeder_transcode(
                        &upstream,
                        Arc::clone(&fan_feed),
                        Arc::clone(&stop_feed),
                    ),
                };
                if stop_feed.load(Ordering::SeqCst) {
                    break;
                }
                match result {
                    Ok(()) => log::warn!("StreamRelay feeder ended; reconnecting"),
                    Err(e) => log::warn!("StreamRelay feeder end: {e}; reconnecting"),
                }
                let delay = if started.elapsed() >= Duration::from_secs(30) {
                    Duration::from_secs(1)
                } else {
                    Duration::from_secs(2)
                };
                let deadline = Instant::now() + delay;
                while !stop_feed.load(Ordering::SeqCst) && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(100));
                }
            }
        });

        let stop_c = Arc::clone(&stop);
        let fan_c = Arc::clone(&fanout);
        let transport_serve = transport.clone();
        log::info!(
            "StreamRelay::start advertise={advertise}:{port} upstream={upstream_url} content-type={content_type}"
        );

        let accept = thread::spawn(move || {
            let _worker = crate::profile::worker("relay_accept");
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

    pub fn tap_url(&self) -> Option<String> {
        self.session.lock().as_ref().and_then(|s| s.tap_url.clone())
    }

    pub fn latest_title(&self) -> Option<String> {
        self.session
            .lock()
            .as_ref()
            .and_then(|s| s.fanout.take_title())
    }

    pub fn levels(&self) -> [f32; crate::audio::spectrum::BANDS] {
        self.session
            .lock()
            .as_ref()
            .map(|s| s.fanout.snapshot_levels())
            .unwrap_or([0.08; crate::audio::spectrum::BANDS])
    }

    pub fn wait_for_pcm_format(&self, timeout: Duration) -> bool {
        let Some(session) = self.session.lock().as_ref().map(|s| Arc::clone(&s.fanout)) else {
            return false;
        };
        let deadline = Instant::now() + timeout;
        loop {
            if session.pcm_format().is_some() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

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

fn detach_relay_worker(join: Option<thread::JoinHandle<()>>, label: &'static str) {
    let Some(join) = join else {
        return;
    };
    thread::spawn(move || {
        let _worker = crate::profile::worker("relay_join");
        let started = Instant::now();
        if let Err(payload) = join.join() {
            log::warn!("StreamRelay {label} worker panicked: {payload:?}");
            return;
        }
        let ms = started.elapsed().as_millis();
        if ms > 3_000 {
            log::warn!("StreamRelay {label} worker joined after {ms}ms");
        }
    });
}

fn port_from_url(url: &str) -> Option<u16> {
    let rest = url.strip_prefix("http://")?;
    let hostport = rest.split('/').next()?;
    let port = hostport.rsplit_once(':')?.1;
    port.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::transport::{
        RelayTransport, TranscodeKind, choose_transport, normalize_content_type,
        tap_url_from_public,
    };
    use super::wav::{stream_response_headers, wav_live_header};

    #[test]
    fn live_stream_response_uses_chunked_encoding() {
        let headers = stream_response_headers("audio/mpeg");
        assert!(headers.contains("Transfer-Encoding: chunked\r\n"));
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
    fn opus_stream_uses_wav_transport() {
        match choose_transport("audio/ogg; codecs=opus", "http://x/stream.opus") {
            RelayTransport::WavPcm {
                kind: TranscodeKind::Opus,
                sample_rate,
                channels,
                ..
            } => {
                assert_eq!(sample_rate, 48_000);
                assert_eq!(channels, 2);
            }
            RelayTransport::WavPcm { .. } | RelayTransport::Passthrough { .. } => {
                panic!("expected opus wav transcode")
            }
        }
    }

    #[test]
    fn aac_stream_uses_pcm_tap_transport() {
        match choose_transport("audio/aacp", "http://x/stream") {
            RelayTransport::WavPcm {
                kind: TranscodeKind::AacAdts,
                sample_rate,
                channels,
                ..
            } => {
                assert_eq!(sample_rate, 48_000);
                assert_eq!(channels, 2);
            }
            RelayTransport::WavPcm { .. } | RelayTransport::Passthrough { .. } => {
                panic!("expected aac pcm transcode")
            }
        }
    }

    #[test]
    fn mp3_stream_uses_pcm_tap_transport() {
        match choose_transport("audio/mpeg", "http://x/stream.mp3") {
            RelayTransport::WavPcm {
                kind: TranscodeKind::Mp3,
                sample_rate,
                channels,
            } => {
                assert_eq!(sample_rate, 48_000);
                assert_eq!(channels, 2);
            }
            RelayTransport::WavPcm { .. } | RelayTransport::Passthrough { .. } => {
                panic!("expected mp3 wav transcode")
            }
        }
    }

    #[test]
    fn flac_stream_uses_wav_transport() {
        match choose_transport("audio/ogg", "http://x/bdpstrock_FLAC") {
            RelayTransport::WavPcm {
                kind: TranscodeKind::Symphonia,
                sample_rate,
                channels,
            } => {
                assert_eq!(sample_rate, 48_000);
                assert_eq!(channels, 2);
            }
            RelayTransport::WavPcm { .. } | RelayTransport::Passthrough { .. } => {
                panic!("expected symphonia wav transcode")
            }
        }
    }

    #[test]
    fn wav_header_looks_valid() {
        let header = wav_live_header(48_000, 2);
        assert_eq!(&header[0..4], b"RIFF");
        assert_eq!(&header[8..12], b"WAVE");
    }
}
