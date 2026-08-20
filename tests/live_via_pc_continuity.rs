use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::Mutex,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use rockcast::relay::StreamRelay;

static ENV_LOCK: Mutex<()> = Mutex::new(());
const AVTORADIO_OPUS_URL: &str = "http://play.global.audio/avtoradio.opus";

fn open_stream(url: &str) -> (TcpStream, usize) {
    let rest = url.strip_prefix("http://").expect("http url");
    let host_port = rest.split('/').next().expect("host:port");
    let mut stream = TcpStream::connect(host_port).expect("connect relay");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set read timeout");
    let request = format!(
        "GET /stream HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).expect("write request");

    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut chunk).expect("read headers");
        assert!(n > 0, "relay closed before headers completed");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let header_end = pos + 4;
            if buf.len() >= header_end + 44 {
                break header_end;
            }
        }
    };

    let headers = String::from_utf8_lossy(&buf[..header_end]);
    assert!(headers.contains("Content-Type: audio/wav\r\n"));
    assert!(buf.len() >= header_end + 44, "missing full wav header");
    assert_eq!(&buf[header_end..header_end + 4], b"RIFF");
    assert_eq!(&buf[header_end + 8..header_end + 12], b"WAVE");

    (stream, buf.len() - (header_end + 44))
}

#[test]
#[ignore = "uses live avtoradio opus stream for 20s continuity probe"]
fn live_via_pc_stream_stays_alive_for_twenty_seconds_and_feeds_spectrum() {
    let _ = env_logger::builder().is_test(true).try_init();
    let _guard = ENV_LOCK.lock().expect("env lock");
    unsafe {
        std::env::set_var("ROCKCAST_RELAY_ADVERTISE_IP", "127.0.0.1");
    }

    let relay = StreamRelay::new();
    let cancel = AtomicBool::new(false);
    let (public_url, content_type) = relay
        .start(
            AVTORADIO_OPUS_URL,
            "127.0.0.1",
            "audio/ogg; codecs=opus",
            &cancel,
        )
        .expect("start relay");

    assert_eq!(content_type, "audio/wav");
    let tap_url = relay.tap_url().expect("tap url");
    assert_ne!(tap_url, public_url);
    assert!(tap_url.ends_with("/tap"));
    assert!(
        relay.wait_for_data(64 * 1024, Duration::from_secs(15)),
        "relay never produced enough decoded PCM"
    );

    let (mut stream, mut audio_bytes) = open_stream(&public_url);
    let start = Instant::now();
    let mut per_second = Vec::new();
    let mut moved = false;

    while start.elapsed() < Duration::from_secs(20) {
        let slice_start = Instant::now();
        let before = audio_bytes;
        while slice_start.elapsed() < Duration::from_secs(1) {
            let mut chunk = [0u8; 16 * 1024];
            match stream.read(&mut chunk) {
                Ok(0) => panic!("relay stream closed after {} bytes", audio_bytes),
                Ok(n) => audio_bytes += n,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => panic!("relay stream read failed: {e}"),
            }
            let levels = relay.levels();
            if levels.iter().any(|level| (level - 0.08).abs() > 0.03) {
                moved = true;
            }
        }
        let second_bytes = audio_bytes - before;
        eprintln!("relay second {}: {} bytes", per_second.len() + 1, second_bytes);
        eprintln!("relay spectrum levels: {:?}", relay.levels());
        per_second.push(second_bytes);
    }

    relay.stop();
    unsafe {
        std::env::remove_var("ROCKCAST_RELAY_ADVERTISE_IP");
    }

    let total_after_header = audio_bytes;
    let quiet_seconds = per_second.iter().filter(|bytes| **bytes < 8192).count();
    assert!(moved, "spectrum never moved on decoded relay stream");
    assert!(
        total_after_header > 1_000_000,
        "relay produced too little audio over 20s: {total_after_header} bytes, per-second={per_second:?}"
    );
    assert!(
        quiet_seconds <= 1,
        "relay stalled too often over 20s: quiet_seconds={quiet_seconds}, per-second={per_second:?}"
    );
}
