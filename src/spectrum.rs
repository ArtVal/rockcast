//! Spectrum analyzer: one HTTP stream → ICY metadata + FFT bands.

use std::{
    collections::VecDeque,
    io::Cursor,
    io::{self, Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use ropus::{Channels as OpusChannels, DecodeMode, Decoder as OpusDecoder};
use symphonia::core::{
    audio::{AudioBufferRef, SampleBuffer},
    codecs::{CODEC_TYPE_NULL, DecoderOptions},
    errors::Error as SymError,
    formats::FormatOptions,
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
    probe::Hint,
};

pub use crate::audio::BANDS;
use crate::audio::{BandAnalyzer, apply_hint, parse_stream_title};
use crate::net::{metadata_interval, stream_client, stream_headers};
/// Streaming FFT levels consumed by the UI at display refresh rate.
pub struct SpectrumAnalyzer {
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
    levels: Arc<Mutex<[f32; BANDS]>>,
}

impl Default for SpectrumAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl SpectrumAnalyzer {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(true)),
            join: None,
            levels: Arc::new(Mutex::new([0.08; BANDS])),
        }
    }

    pub fn levels(&self) -> [f32; BANDS] {
        *self.levels.lock()
    }

    /// `title_tx` — optional StreamTitle from the same connection (instead of a separate ICY).
    pub fn start(&mut self, url: String, title_tx: Option<mpsc::Sender<String>>) {
        self.stop_async();
        *self.levels.lock() = [0.08; BANDS];
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Arc::clone(&stop);
        let levels = Arc::clone(&self.levels);
        self.join = Some(thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                if let Err(e) = analyze_stream(&url, &levels, &stop, title_tx.as_ref()) {
                    log::debug!("spectrum: {e}");
                }
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let until = Instant::now() + Duration::from_secs(2);
                while Instant::now() < until {
                    if stop.load(Ordering::SeqCst) {
                        *levels.lock() = [0.08; BANDS];
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
            *levels.lock() = [0.08; BANDS];
        }));
    }

    pub fn stop_async(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            drop(j);
        }
        *self.levels.lock() = [0.08; BANDS];
    }
}

impl Drop for SpectrumAnalyzer {
    fn drop(&mut self) {
        self.stop_async();
    }
}

struct IcyStripReader {
    inner: reqwest::blocking::Response,
    meta_int: usize,
    until_meta: usize,
    stop: Arc<AtomicBool>,
    title_tx: Option<mpsc::Sender<String>>,
    last_title: String,
}

impl IcyStripReader {
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
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

impl Read for IcyStripReader {
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

impl Seek for IcyStripReader {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "stream is not seekable",
        ))
    }
}

impl MediaSource for IcyStripReader {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

fn analyze_stream(
    url: &str,
    levels: &Mutex<[f32; BANDS]>,
    stop: &Arc<AtomicBool>,
    title_tx: Option<&mpsc::Sender<String>>,
) -> Result<(), String> {
    if stop.load(Ordering::SeqCst) {
        return Err("stopped".into());
    }

    // A short read-timeout via the overall request timeout won't work for a live stream.
    // connect_timeout + periodic stop checks in read; overall timeout guards against hangs.
    let client = stream_client(Duration::from_secs(4), Some(Duration::from_secs(45)))?;

    let resp = client
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

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if let Some((sample_rate, channels)) = raw_pcm_params(resp.headers(), &content_type) {
        return analyze_pcm_stream(resp, levels, stop, sample_rate, channels);
    }
    if looks_like_opus(url, &content_type) {
        return analyze_opus_stream(resp, levels, stop);
    }

    let meta_int = metadata_interval(resp.headers());

    let source = IcyStripReader {
        inner: resp,
        meta_int,
        until_meta: if meta_int == 0 { usize::MAX } else { meta_int },
        stop: Arc::clone(stop),
        title_tx: title_tx.cloned(),
        last_title: String::new(),
    };

    let mss = MediaSourceStream::new(Box::new(source), Default::default());
    let mut hint = Hint::new();
    apply_hint(&mut hint, &content_type);

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions {
                enable_gapless: false,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|e| e.to_string())?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| "no audio track".to_string())?
        .clone();

    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44_100) as f32;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| e.to_string())?;

    let mut bands = BandAnalyzer::new();
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    let wall_start = Instant::now();
    let mut samples_done: u64 = 0;

    while !stop.load(Ordering::SeqCst) {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::ResetRequired) => return Err("reset".into()),
            Err(SymError::IoError(e))
                if e.kind() == io::ErrorKind::UnexpectedEof
                    || e.kind() == io::ErrorKind::Interrupted =>
            {
                return Err("eof".into());
            }
            Err(_) if stop.load(Ordering::SeqCst) => return Err("stopped".into()),
            Err(e) => return Err(e.to_string()),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(e.to_string()),
        };

        let mut mono = Vec::new();
        let frames = push_mono(&decoded, &mut sample_buf, &mut mono);
        samples_done += frames as u64;
        if let Some(values) = bands.push_mono(mono, sample_rate) {
            *levels.lock() = values;
        }

        // Don't outpace realtime — otherwise 100% CPU on a buffered stream.
        let audio_secs = samples_done as f64 / f64::from(sample_rate);
        let elapsed = wall_start.elapsed().as_secs_f64();
        if audio_secs > elapsed + 0.08 {
            let ms = ((audio_secs - elapsed - 0.03) * 1000.0).clamp(1.0, 80.0) as u64;
            let until = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < until {
                if stop.load(Ordering::SeqCst) {
                    return Err("stopped".into());
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    Ok(())
}

fn looks_like_opus(url: &str, content_type: &str) -> bool {
    let u = url.to_ascii_lowercase();
    let ct = content_type.to_ascii_lowercase();
    u.ends_with(".opus")
        || u.contains(".opus?")
        || ct.contains("codecs=opus")
        || ct.contains("audio/opus")
        || ct == "application/ogg"
}

fn raw_pcm_params(headers: &reqwest::header::HeaderMap, content_type: &str) -> Option<(f32, usize)> {
    let ct = content_type.to_ascii_lowercase();
    let is_pcm = ct.contains("audio/l16")
        || ct.contains("audio/lpcm")
        || ct.contains("audio/pcm")
        || headers.contains_key("x-audio-sample-rate");
    if !is_pcm {
        return None;
    }
    let sample_rate = headers
        .get("x-audio-sample-rate")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(48_000.0);
    let channels = headers
        .get("x-audio-channels")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2)
        .max(1);
    Some((sample_rate, channels))
}

fn analyze_pcm_stream(
    mut resp: reqwest::blocking::Response,
    levels: &Mutex<[f32; BANDS]>,
    stop: &Arc<AtomicBool>,
    sample_rate: f32,
    channels: usize,
) -> Result<(), String> {
    let mut bands = BandAnalyzer::new();
    let mut buf = vec![0u8; 16 * 1024];
    let mut pending = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        let n = match resp.read(&mut buf) {
            Ok(0) => return Err("eof".into()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        };
        pending.extend_from_slice(&buf[..n]);
        let complete = pending.len() / 2 * 2;
        if complete == 0 {
            continue;
        }
        let mut mono = Vec::with_capacity(complete / (2 * channels.max(1)));
        for frame in pending[..complete].chunks_exact(2 * channels) {
            let mut sum = 0.0f32;
            for sample in frame.chunks_exact(2).take(channels) {
                let value = i16::from_le_bytes([sample[0], sample[1]]);
                sum += f32::from(value) / f32::from(i16::MAX);
            }
            mono.push(sum / channels as f32);
        }
        pending.drain(..complete);
        if let Some(values) = bands.push_mono(mono, sample_rate) {
            *levels.lock() = values;
        }
    }
    Err("stopped".into())
}

fn analyze_opus_stream(
    resp: reqwest::blocking::Response,
    levels: &Mutex<[f32; BANDS]>,
    stop: &Arc<AtomicBool>,
) -> Result<(), String> {
    let mut reader = LiveOggOpusReader::new(resp);
    let mut decoder =
        OpusDecoder::new(48_000, OpusChannels::Stereo).map_err(|e| format!("opus decoder: {e}"))?;
    let mut pcm = vec![0i16; 5760 * 2];
    let mut bands = BandAnalyzer::new();
    let mut mono = Vec::with_capacity(5760);
    while !stop.load(Ordering::SeqCst) {
        let packet = match reader.read_packet(stop)? {
            Some(packet) => packet,
            None => return Err("eof".into()),
        };
        let samples = match decoder.decode(&packet, &mut pcm, DecodeMode::Normal) {
            Ok(samples) => samples,
            Err(e) => {
                log::debug!("spectrum opus decode error: {e}");
                continue;
            }
        };
        if samples == 0 {
            continue;
        }
        mono.clear();
        for frame in pcm[..samples * 2].chunks_exact(2) {
            let l = f32::from(frame[0]) / f32::from(i16::MAX);
            let r = f32::from(frame[1]) / f32::from(i16::MAX);
            mono.push((l + r) * 0.5);
        }
        if let Some(values) = bands.push_mono(std::mem::take(&mut mono), 48_000.0) {
            *levels.lock() = values;
        }
    }
    Err("stopped".into())
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
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
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

fn push_mono(
    decoded: &AudioBufferRef<'_>,
    sample_buf: &mut Option<SampleBuffer<f32>>,
    pcm: &mut Vec<f32>,
) -> usize {
    let spec = *decoded.spec();
    let frames = decoded.frames();
    if sample_buf.as_ref().is_none_or(|b| b.capacity() < frames) {
        *sample_buf = Some(SampleBuffer::<f32>::new(frames as u64, spec));
    }
    let buf = sample_buf.as_mut().unwrap();
    buf.copy_interleaved_ref(decoded.clone());
    let samples = buf.samples();
    let ch = spec.channels.count().max(1);
    let before = pcm.len();
    for frame in samples.chunks(ch) {
        let sum: f32 = frame.iter().sum();
        pcm.push(sum / ch as f32);
    }
    pcm.len() - before
}
