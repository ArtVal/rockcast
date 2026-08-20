use std::{
    io::{Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::audio::format::is_mp3_stream;

use super::{
    fanout::{Fanout, RING_MAX},
    transport::RelayTransport,
    wav::{
        pcm_tap_response_headers, stream_response_headers, tap_response_headers, wav_live_header,
        wav_response_headers,
    },
};

const ACCEPT_POLL: Duration = Duration::from_millis(200);
const BUF: usize = 16 * 1024;

pub fn accept_loop(
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
    if !is_get || (!is_stream && !is_tap) {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n");
        return Err(format!("bad request: {first}"));
    }

    let resolved_pcm = fanout.pcm_format().or_else(|| match transport {
        RelayTransport::WavPcm {
            sample_rate,
            channels,
            ..
        } => Some((*sample_rate, *channels)),
        RelayTransport::Passthrough { .. } => None,
    });

    let headers = if is_tap {
        match transport {
            RelayTransport::Passthrough { content_type } => tap_response_headers(content_type),
            RelayTransport::WavPcm {
                sample_rate,
                channels,
                ..
            } => {
                let (sample_rate, channels) = resolved_pcm.unwrap_or((*sample_rate, *channels));
                pcm_tap_response_headers(sample_rate, channels)
            }
        }
    } else {
        match transport {
            RelayTransport::Passthrough { content_type } => stream_response_headers(content_type),
            RelayTransport::WavPcm {
                sample_rate,
                channels,
                ..
            } => {
                let (sample_rate, channels) = resolved_pcm.unwrap_or((*sample_rate, *channels));
                wav_response_headers(sample_rate, channels)
            }
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
            ..
        } = transport
    {
        let (sample_rate, channels) = resolved_pcm.unwrap_or((*sample_rate, *channels));
        let header = wav_live_header(sample_rate, channels);
        write_all_retry(&mut stream, &header, stop)?;
    }

    let mut pos = fanout.join_position();
    let content_type = match transport {
        RelayTransport::Passthrough { content_type } => content_type.as_str(),
        RelayTransport::WavPcm { .. } => "audio/wav",
    };
    let mut peek = [0u8; 4_096];
    let peek_len = fanout.copy_at(pos, &mut peek);
    if is_mp3_stream("", content_type, &peek[..peek_len]) {
        pos = fanout.align_mp3_pos(pos);
    } else if matches!(transport, RelayTransport::WavPcm { .. }) {
        pos = fanout.align_pcm_pos(pos);
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

#[allow(dead_code)]
const _: () = assert!(RING_MAX > 0);
