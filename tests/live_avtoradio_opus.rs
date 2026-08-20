use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{Mutex, atomic::AtomicBool},
    thread,
    time::{Duration, Instant},
};

use rockcast::{observers::SpectrumAnalyzer, relay::StreamRelay};

static ENV_LOCK: Mutex<()> = Mutex::new(());
const AVTORADIO_OPUS_URL: &str = "http://play.global.audio/avtoradio.opus";

fn connect_and_read(url: &str) -> Vec<u8> {
    let rest = url.strip_prefix("http://").expect("http url");
    let host_port = rest.split('/').next().expect("host:port");
    let mut stream = TcpStream::connect(host_port).expect("connect relay");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    let request = format!("GET /stream HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
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
#[ignore = "uses live avtoradio opus stream"]
fn live_relay_decodes_avtoradio_opus_into_wav() {
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
    assert!(
        relay.wait_for_data(32 * 1024, Duration::from_secs(15)),
        "relay did not decode enough PCM from live opus stream"
    );

    let response = connect_and_read(&public_url);
    let header_end = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("http headers")
        + 4;
    let headers = String::from_utf8_lossy(&response[..header_end]);
    assert!(headers.contains("Content-Type: audio/wav\r\n"));
    assert!(
        response.len() >= header_end + 44,
        "short wav response: {} bytes",
        response.len()
    );
    assert_eq!(&response[header_end..header_end + 4], b"RIFF");
    assert_eq!(&response[header_end + 8..header_end + 12], b"WAVE");

    relay.stop();
    unsafe {
        std::env::remove_var("ROCKCAST_RELAY_ADVERTISE_IP");
    }
}

#[test]
#[ignore = "uses live avtoradio opus stream"]
fn live_spectrum_moves_on_avtoradio_opus() {
    let mut spectrum = SpectrumAnalyzer::new();
    spectrum.start(AVTORADIO_OPUS_URL.to_string(), None);

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut moved = false;
    while Instant::now() < deadline {
        let levels = spectrum.levels();
        if levels.iter().any(|level| (level - 0.08).abs() > 0.03) {
            moved = true;
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }

    spectrum.stop_async();
    assert!(
        moved,
        "spectrum levels never moved on live avtoradio opus stream"
    );
}
