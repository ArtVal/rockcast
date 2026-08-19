use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    audio::{
        decode::run_live_decode_relay_pcm,
        format::parse_stream_title,
        spectrum::SpectrumTap,
    },
    net::{metadata_interval, stream_client, stream_headers},
};

use super::fanout::{Fanout, RING_MAX};

const OPEN_TIMEOUT: Duration = Duration::from_secs(15);
const READ_POLL: Duration = Duration::from_millis(200);
const BUF: usize = 16 * 1024;
const FANOUT_HIGH_WATER: usize = RING_MAX - 512 * 1024;

pub fn run_feeder_passthrough(url: &str, fanout: &Fanout, stop: &AtomicBool) -> Result<(), String> {
    let headers = stream_headers(false);
    let client = stream_client(Duration::from_secs(10), None)?;
    let resp = open_upstream(client, url, headers, stop)?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let meta_int = metadata_interval(resp.headers());
    log::info!(
        "StreamRelay passthrough feeder HTTP ok content-type={} icy-metaint={}",
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

pub fn run_feeder_transcode(
    url: &str,
    fanout: Arc<Fanout>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    let mut spectrum = SpectrumTap::new(fanout.levels());
    let fan_push = Arc::clone(&fanout);
    let stop_push = Arc::clone(&stop);
    run_live_decode_relay_pcm(
        url,
        &stop,
        &mut spectrum,
        move |chunk| {
            while fan_push.buffered_bytes() > FANOUT_HIGH_WATER && !stop_push.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(2));
            }
            fan_push.push(chunk);
        },
        move |rate, ch| fanout.set_pcm_format(rate, ch),
    )
}

fn open_upstream(
    client: reqwest::blocking::Client,
    url: &str,
    headers: reqwest::header::HeaderMap,
    stop: &AtomicBool,
) -> Result<reqwest::blocking::Response, String> {
    let url = url.to_string();
    let (tx, rx) = mpsc::channel();
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
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("failed to open upstream".into());
            }
        }
    }
}
