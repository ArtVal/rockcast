//! ICY metadata stripping for live HTTP streams.

use std::{
    io::{self, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use crate::audio::format::parse_stream_title;

const READ_POLL: Duration = Duration::from_millis(200);

pub struct IcyStreamReader<R: Read> {
    inner: R,
    meta_int: usize,
    until_meta: usize,
    stop: Arc<AtomicBool>,
    title_tx: Option<mpsc::Sender<String>>,
    last_title: String,
}

impl<R: Read> IcyStreamReader<R> {
    pub fn new(
        inner: R,
        meta_int: usize,
        stop: Arc<AtomicBool>,
        title_tx: Option<mpsc::Sender<String>>,
    ) -> Self {
        Self {
            inner,
            meta_int,
            until_meta: meta_int,
            stop,
            title_tx,
            last_title: String::new(),
        }
    }

    fn skip_meta(&mut self) -> io::Result<()> {
        let mut len_byte = [0u8; 1];
        self.read_exact_stop(&mut len_byte)?;
        let meta_len = (len_byte[0] as usize) * 16;
        if meta_len > 0 {
            let mut meta = vec![0u8; meta_len];
            self.read_exact_stop(&mut meta)?;
            if let Some(tx) = &self.title_tx
                && let Some(title) = parse_stream_title(&meta)
            {
                let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
                if !title.is_empty() && title != self.last_title {
                    self.last_title = title.clone();
                    let _ = tx.send(title);
                }
            }
        }
        self.until_meta = self.meta_int;
        Ok(())
    }

    fn read_exact_stop(&mut self, buf: &mut [u8]) -> io::Result<()> {
        let mut got = 0;
        while got < buf.len() {
            if self.stop.load(Ordering::SeqCst) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "stopped"));
            }
            match self.inner.read(&mut buf[got..]) {
                Ok(0) => {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof"));
                }
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    if self.stop.load(Ordering::SeqCst) {
                        return Err(e);
                    }
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

impl<R: Read> Read for IcyStreamReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.stop.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "stopped"));
        }
        if self.meta_int == 0 {
            return self.inner.read(buf);
        }
        if self.until_meta == 0 {
            self.skip_meta()?;
        }
        let max = buf.len().min(self.until_meta);
        if max == 0 {
            return Ok(0);
        }
        let n = self.inner.read(&mut buf[..max])?;
        if n == 0 {
            return Ok(0);
        }
        self.until_meta = self.until_meta.saturating_sub(n);
        Ok(n)
    }
}

/// Reads HTTP body on a side thread; `stop` ends the producer (channel closes).
pub struct StopAwareBody {
    rx: mpsc::Receiver<io::Result<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    pending: Vec<u8>,
    pending_at: usize,
}

impl StopAwareBody {
    pub fn spawn(resp: reqwest::blocking::Response, stop: Arc<AtomicBool>) -> Self {
        let (tx, rx) = mpsc::sync_channel(32);
        let stop_prod = Arc::clone(&stop);
        std::thread::spawn(move || {
            let (read_tx, read_rx) = mpsc::sync_channel::<io::Result<Option<Vec<u8>>>>(2);
            std::thread::spawn(move || {
                let mut resp = resp;
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    match resp.read(&mut buf) {
                        Ok(0) => {
                            let _ = read_tx.send(Ok(None));
                            break;
                        }
                        Ok(n) => {
                            if read_tx
                                .send(Ok(Some(buf[..n].to_vec())))
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(e) => {
                            let _ = read_tx.send(Err(e));
                            break;
                        }
                    }
                }
            });
            loop {
                if stop_prod.load(Ordering::SeqCst) {
                    break;
                }
                match read_rx.recv_timeout(READ_POLL) {
                    Ok(Ok(None)) => {
                        let _ = tx.try_send(Ok(Vec::new()));
                        break;
                    }
                    Ok(Ok(Some(chunk))) => {
                        let mut msg = Ok(chunk);
                        loop {
                            if stop_prod.load(Ordering::SeqCst) {
                                return;
                            }
                            match tx.try_send(msg) {
                                Ok(()) => break,
                                Err(mpsc::TrySendError::Full(m)) => {
                                    msg = m;
                                    std::thread::sleep(Duration::from_millis(25));
                                }
                                Err(mpsc::TrySendError::Disconnected(_)) => return,
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        let _ = tx.try_send(Err(e));
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        Self {
            rx,
            stop,
            pending: Vec::new(),
            pending_at: 0,
        }
    }
}

impl Read for StopAwareBody {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "stopped"));
            }
            if self.pending_at < self.pending.len() {
                let n = (self.pending.len() - self.pending_at).min(buf.len());
                buf[..n].copy_from_slice(&self.pending[self.pending_at..self.pending_at + n]);
                self.pending_at += n;
                if self.pending_at >= self.pending.len() {
                    self.pending.clear();
                    self.pending_at = 0;
                }
                return Ok(n);
            }
            let chunk = match self.rx.recv_timeout(READ_POLL) {
                Ok(Ok(chunk)) if chunk.is_empty() => return Ok(0),
                Ok(Ok(chunk)) => chunk,
                Ok(Err(e)) => return Err(e),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(0),
            };
            self.pending = chunk;
            self.pending_at = 0;
        }
    }
}

const OPEN_TIMEOUT: Duration = Duration::from_secs(12);

pub fn open_stream_response(
    client: reqwest::blocking::Client,
    url: &str,
    headers: reqwest::header::HeaderMap,
    stop: &Arc<AtomicBool>,
) -> Result<reqwest::blocking::Response, String> {
    let url = url.to_string();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
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
            return Err("stream open timeout".into());
        }
        match rx.recv_timeout(wait) {
            Ok(Ok(resp)) => return Ok(resp),
            Ok(Err(e)) => return Err(e.to_string()),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("failed to open audio stream".into());
            }
        }
    }
}
