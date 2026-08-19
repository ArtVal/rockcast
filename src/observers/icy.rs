//! Read ICY/Shoutcast metadata (current track) from an audio stream.

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

use crate::audio::format::parse_stream_title;
use crate::net::{metadata_interval, stream_client, stream_headers};

pub struct IcyWatcher {
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl Default for IcyWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl IcyWatcher {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(true)),
            join: None,
        }
    }

    pub fn start(&mut self, url: String, tx: mpsc::Sender<String>) {
        self.stop_async();
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Arc::clone(&stop);
        self.join = Some(thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                if let Err(e) = listen_once(&url, &tx, &stop) {
                    log::debug!("icy: {e}");
                }
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                // Short pause with stop checks — UI may switch stations immediately.
                let until = Instant::now() + Duration::from_secs(3);
                while Instant::now() < until {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }));
    }

    /// Stops the watcher without blocking the UI thread.
    pub fn stop_async(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            // The request has a finite timeout and observes `stop`; detaching
            // avoids creating an additional joiner thread for every UI action.
            drop(j);
        }
    }
}

impl Drop for IcyWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            // On exit wait briefly, but not forever.
            let _ = j.join();
        }
    }
}

fn listen_once(url: &str, tx: &mpsc::Sender<String>, stop: &AtomicBool) -> Result<(), String> {
    crate::profile::bump("icy_connect");
    if stop.load(Ordering::SeqCst) {
        return Err("stopped".into());
    }

    let client = stream_client(Duration::from_secs(4), Some(Duration::from_secs(20)))?;

    let mut resp = client
        .get(url)
        .headers(stream_headers(true))
        .send()
        .map_err(|e| e.to_string())?;
    if stop.load(Ordering::SeqCst) {
        return Err("stopped".into());
    }
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let meta_int = metadata_interval(resp.headers());

    if meta_int == 0 {
        if let Some(name) = resp
            .headers()
            .get("icy-name")
            .or_else(|| resp.headers().get("ice-name"))
            .and_then(|v| v.to_str().ok())
        {
            let _ = tx.send(name.trim().to_string());
        }
        // No ICY metaint — don't keep the stream open; UI shouldn't wait forever for metadata.
        return Ok(());
    }

    let mut last = String::new();
    let deadline = Instant::now() + Duration::from_secs(45);
    while !stop.load(Ordering::SeqCst) && Instant::now() < deadline {
        let mut audio = vec![0u8; meta_int];
        read_interruptible(&mut resp, &mut audio, stop)?;
        let mut len_byte = [0u8; 1];
        read_interruptible(&mut resp, &mut len_byte, stop)?;
        let meta_len = (len_byte[0] as usize) * 16;
        if meta_len == 0 {
            continue;
        }
        let mut meta = vec![0u8; meta_len];
        read_interruptible(&mut resp, &mut meta, stop)?;
        if let Some(title) = parse_stream_title(&meta) {
            let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
            if !title.is_empty() && title != last {
                last = title.clone();
                let _ = tx.send(title);
            }
        }
    }
    Ok(())
}

fn read_interruptible(
    reader: &mut impl Read,
    buf: &mut [u8],
    stop: &AtomicBool,
) -> Result<(), String> {
    let mut got = 0;
    while got < buf.len() {
        if stop.load(Ordering::SeqCst) {
            return Err("stopped".into());
        }
        match reader.read(&mut buf[got..]) {
            Ok(0) => return Err("eof".into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                if stop.load(Ordering::SeqCst) {
                    return Err("stopped".into());
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
