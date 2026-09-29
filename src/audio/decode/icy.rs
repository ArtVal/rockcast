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
                    log::info!("stream title: {title}");
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
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "stopped"));
            }
            match self.inner.read(&mut buf[got..]) {
                Ok(0) => {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof"));
                }
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    if self.stop.load(Ordering::SeqCst) {
                        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "stopped"));
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
            return Ok(0);
        }
        if self.meta_int == 0 {
            return match self.inner.read(buf) {
                Ok(n) => Ok(n),
                Err(_) if self.stop.load(Ordering::SeqCst) => Ok(0),
                Err(e) => Err(e),
            };
        }
        if self.until_meta == 0 {
            if let Err(e) = self.skip_meta() {
                if self.stop.load(Ordering::SeqCst) {
                    return Ok(0);
                }
                return Err(e);
            }
        }
        let max = buf.len().min(self.until_meta);
        if max == 0 {
            return Ok(0);
        }
        let n = match self.inner.read(&mut buf[..max]) {
            Ok(n) => n,
            Err(_) if self.stop.load(Ordering::SeqCst) => return Ok(0),
            Err(e) => return Err(e),
        };
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
            let _dispatch_worker = crate::profile::worker("http_body_dispatch");
            let (read_tx, read_rx) = mpsc::sync_channel::<io::Result<Option<Vec<u8>>>>(2);
            let stop_read = Arc::clone(&stop_prod);
            std::thread::spawn(move || {
                let _read_worker = crate::profile::worker("http_body_read");
                let mut resp = resp;
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    if stop_read.load(Ordering::SeqCst) {
                        break;
                    }
                    match resp.read(&mut buf) {
                        Ok(0) => {
                            let _ = read_tx.send(Ok(None));
                            break;
                        }
                        Ok(n) => {
                            if read_tx.send(Ok(Some(buf[..n].to_vec()))).is_err() {
                                break;
                            }
                        }
                        Err(e)
                            if e.kind() == io::ErrorKind::Interrupted
                                || e.kind() == io::ErrorKind::WouldBlock
                                || e.kind() == io::ErrorKind::TimedOut =>
                        {
                            if stop_read.load(Ordering::SeqCst) {
                                break;
                            }
                            continue;
                        }
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
                return Ok(0);
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
                Ok(Err(e)) => {
                    if self.stop.load(Ordering::SeqCst) {
                        return Ok(0);
                    }
                    return Err(e);
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn icy_stream_reader_returns_eof_when_stopped() {
        let data = vec![0xAB; 1024];
        let stop = Arc::new(AtomicBool::new(false));
        let mut reader = IcyStreamReader::new(Cursor::new(data), 0, Arc::clone(&stop), None);

        let mut buf = [0u8; 10];
        assert_eq!(reader.read(&mut buf).unwrap(), 10);

        stop.store(true, Ordering::SeqCst);
        assert_eq!(reader.read(&mut buf).unwrap(), 0);

        // Subsequent reads must return Ok(0) immediately
        assert_eq!(reader.read(&mut buf).unwrap(), 0);

        // Standard read_exact must fail with UnexpectedEof immediately rather than looping forever
        let mut exact_buf = [0u8; 10];
        let err = reader.read_exact(&mut exact_buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn icy_stream_reader_with_metadata_returns_eof_when_stopped() {
        // Stream with 16 bytes of audio, 1 byte meta-length = 1 (16 bytes of metadata), then more audio
        let mut data = vec![0x11; 16];
        data.push(1); // 1 * 16 = 16 bytes of meta
        data.extend_from_slice(b"StreamTitle='A';");
        data.extend(vec![0x22; 32]);

        let stop = Arc::new(AtomicBool::new(false));
        let (title_tx, title_rx) = mpsc::channel();
        let mut reader =
            IcyStreamReader::new(Cursor::new(data), 16, Arc::clone(&stop), Some(title_tx));

        let mut buf = [0u8; 16];
        assert_eq!(reader.read(&mut buf).unwrap(), 16);

        // Next read triggers skip_meta, parses title, and reads audio
        let mut buf2 = [0u8; 8];
        assert_eq!(reader.read(&mut buf2).unwrap(), 8);
        assert_eq!(title_rx.try_recv().unwrap(), "A");

        // Now stop the reader
        stop.store(true, Ordering::SeqCst);
        let mut buf3 = [0u8; 8];
        assert_eq!(reader.read(&mut buf3).unwrap(), 0);
    }
}
