use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::Mutex,
    sync::atomic::AtomicBool,
    time::Duration,
};

use rockcast::relay::StreamRelay;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn connect_and_get(url: &str, path: &str) -> Vec<u8> {
    let rest = url.strip_prefix("http://").expect("http url");
    let host_port = rest.split('/').next().expect("host:port");
    let mut stream = TcpStream::connect(host_port).expect("connect relay");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("write request");

    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() >= 320 {
                    break;
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(e) => panic!("read response: {e}"),
        }
    }
    buf
}

#[test]
fn opus_via_pc_exposes_same_public_url_for_spectrum() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    unsafe {
        std::env::set_var("ROCKCAST_RELAY_ADVERTISE_IP", "127.0.0.1");
    }
    let relay = StreamRelay::new();
    let cancel = AtomicBool::new(false);
    let (public_url, content_type) = relay
        .start(
            "http://127.0.0.1:9/stream.opus",
            "127.0.0.1",
            "audio/ogg; codecs=opus",
            &cancel,
        )
        .expect("start relay");

    assert_eq!(content_type, "audio/wav");
    let expected_tap = public_url.replace("/stream", "/tap");
    assert_eq!(relay.tap_url().as_deref(), Some(expected_tap.as_str()));

    let response = connect_and_get(&public_url, "/stream");
    let header_end = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("http headers")
        + 4;
    let headers = String::from_utf8_lossy(&response[..header_end]);
    assert!(headers.contains("Content-Type: audio/wav\r\n"));
    assert!(headers.contains("Content-Length:"));
    assert!(
        response.len() >= header_end + 12,
        "wav header incomplete: {} bytes",
        response.len()
    );
    assert_eq!(&response[header_end..header_end + 4], b"RIFF");
    assert_eq!(&response[header_end + 8..header_end + 12], b"WAVE");

    let tap_response = connect_and_get(&expected_tap, "/tap");
    let tap_header_end = tap_response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("tap headers")
        + 4;
    let tap_headers = String::from_utf8_lossy(&tap_response[..tap_header_end]);
    assert!(tap_headers.contains("Content-Type: audio/L16\r\n"));
    assert!(tap_headers.contains("X-Audio-Sample-Rate: 48000\r\n"));
    assert!(tap_headers.contains("X-Audio-Channels: 2\r\n"));

    relay.stop();
    unsafe {
        std::env::remove_var("ROCKCAST_RELAY_ADVERTISE_IP");
    }
}

#[test]
fn mp3_via_pc_exposes_pcm_tap_for_spectrum() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    unsafe {
        std::env::set_var("ROCKCAST_RELAY_ADVERTISE_IP", "127.0.0.1");
    }
    let relay = StreamRelay::new();
    let cancel = AtomicBool::new(false);
    let (public_url, content_type) = relay
        .start(
            "http://127.0.0.1:9/stream.mp3",
            "127.0.0.1",
            "audio/mpeg",
            &cancel,
        )
        .expect("start relay");

    assert_eq!(content_type, "audio/wav");
    let expected_tap = public_url.replace("/stream", "/tap");
    assert_eq!(relay.tap_url().as_deref(), Some(expected_tap.as_str()));

    let tap_url = relay.tap_url().expect("tap url");
    let response = connect_and_get(&tap_url, "/tap");
    let header_end = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("http headers")
        + 4;
    let headers = String::from_utf8_lossy(&response[..header_end]);
    assert!(headers.contains("Content-Type: audio/L16\r\n"));
    assert!(!headers.contains("Transfer-Encoding: chunked\r\n"));

    relay.stop();
    unsafe {
        std::env::remove_var("ROCKCAST_RELAY_ADVERTISE_IP");
    }
}
