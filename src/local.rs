//! Local internet-radio playback to PC speakers (cpal + symphonia).

use std::{
    collections::VecDeque,
    io::{self, Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use rustfft::{FftPlanner, num_complex::Complex};
use symphonia::core::{
    audio::{AudioBufferRef, SampleBuffer},
    codecs::{CODEC_TYPE_NULL, DecoderOptions},
    errors::Error as SymError,
    formats::FormatOptions,
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
    probe::Hint,
};
use thiserror::Error;

use crate::spectrum::{BANDS, FFT_SIZE, HOP};

const RING_MAX: usize = 48000 * 2 * 4; // ~4 sec stereo @ 48k
/// Give up if the station accepts TCP but never sends HTTP headers / audio.
const OPEN_TIMEOUT: Duration = Duration::from_secs(12);
const READ_POLL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub struct LocalDeviceInfo {
    pub id: String,
    pub name: String,
    /// cpal device name; `None` — system default.
    pub cpal_name: Option<String>,
}

impl LocalDeviceInfo {
    pub fn label(&self, lang: crate::i18n::Lang) -> String {
        format!("{}  [{}]", self.name, lang.t().this_pc)
    }
}

#[derive(Debug, Error)]
pub enum LocalError {
    #[error("audio: {0}")]
    Audio(String),
    #[error("stream: {0}")]
    Stream(String),
}

pub fn list_local_devices(lang: crate::i18n::Lang) -> Vec<LocalDeviceInfo> {
    let host = cpal::default_host();
    let default_name = host
        .default_output_device()
        .and_then(|d| d.name().ok());
    let speakers = lang.t().pc_speakers;

    let mut out = Vec::new();
    let Ok(devices) = host.output_devices() else {
        out.push(LocalDeviceInfo {
            id: "local:default".into(),
            name: speakers.into(),
            cpal_name: None,
        });
        return out;
    };

    for device in devices {
        let Ok(name) = device.name() else {
            continue;
        };
        let is_default = default_name.as_ref() == Some(&name);
        let display = if is_default {
            format!("{name} ★")
        } else {
            name.clone()
        };
        out.push(LocalDeviceInfo {
            id: format!("local:{name}"),
            name: display,
            cpal_name: Some(name),
        });
    }

    if out.is_empty() {
        out.push(LocalDeviceInfo {
            id: "local:default".into(),
            name: speakers.into(),
            cpal_name: None,
        });
    } else {
        // Default first.
        out.sort_by(|a, b| {
            let ad = a.name.contains('★');
            let bd = b.name.contains('★');
            match (ad, bd) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });
    }
    out
}

fn f32_bits(v: f32) -> u32 {
    v.to_bits()
}

fn bits_f32(b: u32) -> f32 {
    f32::from_bits(b)
}

pub struct LocalPlayer {
    /// Stop flag for the *current* session only. Each `play` gets a fresh Arc so
    /// cancelling an old hung decode cannot be undone by the next `play`.
    session_stop: Mutex<Arc<AtomicBool>>,
    /// Serialize play setup so two concurrent `play` calls cannot race on state.
    play_lock: Mutex<()>,
    state: Mutex<PlayerState>,
    volume: Arc<AtomicU32>,
    levels: Arc<Mutex<[f32; BANDS]>>,
}

struct PlayerState {
    join: Option<thread::JoinHandle<()>>,
    stream: Option<SendStream>,
}

/// cpal marks Stream as !Send for portability; on WASAPI this is safe.
#[allow(dead_code)]
struct SendStream(cpal::Stream);

// SAFETY: Stream lives only inside LocalPlayer; access is serialized via Mutex.
unsafe impl Send for SendStream {}
unsafe impl Sync for SendStream {}

impl Default for LocalPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalPlayer {
    pub fn new() -> Self {
        Self {
            session_stop: Mutex::new(Arc::new(AtomicBool::new(true))),
            play_lock: Mutex::new(()),
            state: Mutex::new(PlayerState {
                join: None,
                stream: None,
            }),
            volume: Arc::new(AtomicU32::new(f32_bits(0.5))),
            levels: Arc::new(Mutex::new([0.08; BANDS])),
        }
    }

    pub fn levels(&self) -> [f32; BANDS] {
        *self.levels.lock()
    }

    pub fn set_volume(&self, level: f32) {
        self.volume
            .store(f32_bits(level.clamp(0.0, 1.0)), Ordering::SeqCst);
    }

    pub fn stop(&self) {
        log::info!("LocalPlayer::stop");
        self.session_stop.lock().store(true, Ordering::SeqCst);
        let (stream, join) = {
            let mut state = self.state.lock();
            (state.stream.take(), state.join.take())
        };
        log::debug!(
            "LocalPlayer::stop: had_stream={} had_join={}",
            stream.is_some(),
            join.is_some()
        );
        // Dropping cpal::Stream / joining a hung HTTP decode can block — never do
        // that on the UI thread.
        if stream.is_some() || join.is_some() {
            thread::spawn(move || {
                log::debug!("LocalPlayer::stop: dropping stream / joining decode");
                drop(stream);
                if let Some(j) = join {
                    let _ = j.join();
                    log::debug!("LocalPlayer::stop: decode join finished");
                }
            });
        }
        *self.levels.lock() = [0.08; BANDS];
    }

    pub fn play(
        &self,
        device: &LocalDeviceInfo,
        url: &str,
        volume: f32,
        title_tx: Option<mpsc::Sender<String>>,
        on_status: impl Fn(&str),
    ) -> Result<(), LocalError> {
        log::info!(
            "LocalPlayer::play begin device='{}' cpal={:?} vol={volume:.2} url={url}",
            device.name,
            device.cpal_name
        );
        self.stop();
        log::debug!("LocalPlayer::play: waiting play_lock");
        let _play_guard = self.play_lock.lock();
        log::debug!("LocalPlayer::play: play_lock acquired");

        self.set_volume(volume);
        on_status(&format!("Local: «{}»...", device.name));

        let stop = Arc::new(AtomicBool::new(false));
        *self.session_stop.lock() = Arc::clone(&stop);
        *self.levels.lock() = [0.08; BANDS];

        let ring = Arc::new(Mutex::new(VecDeque::<f32>::with_capacity(RING_MAX / 4)));
        let src_rate = Arc::new(AtomicU32::new(0));
        let src_ch = Arc::new(AtomicU32::new(2));
        let err_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let url = url.to_string();
        let levels = Arc::clone(&self.levels);
        let ring_dec = Arc::clone(&ring);
        let stop_dec = Arc::clone(&stop);
        let src_rate_c = Arc::clone(&src_rate);
        let src_ch_c = Arc::clone(&src_ch);
        let err_c = Arc::clone(&err_slot);

        {
            let mut state = self.state.lock();
            state.join = Some(thread::spawn(move || {
                log::info!("LocalPlayer decode thread started url={url}");
                let ready = AtomicBool::new(false);
                if let Err(e) = decode_into_ring(
                    &url,
                    &ring_dec,
                    &levels,
                    &stop_dec,
                    &src_rate_c,
                    &src_ch_c,
                    &ready,
                    title_tx.as_ref(),
                ) {
                    if !stop_dec.load(Ordering::SeqCst) {
                        log::warn!("LocalPlayer decode ended with error: {e}");
                        *err_c.lock() = Some(e);
                    } else {
                        log::info!("LocalPlayer decode stopped ({e})");
                    }
                } else {
                    log::info!("LocalPlayer decode thread exited cleanly");
                }
            }));
        }

        // Wait for probe without holding the state mutex — stop() can interrupt.
        let deadline = Instant::now() + OPEN_TIMEOUT;
        while Instant::now() < deadline {
            if let Some(e) = err_slot.lock().clone() {
                log::error!("LocalPlayer::play: decode error while waiting probe: {e}");
                self.stop();
                return Err(LocalError::Stream(e));
            }
            if src_rate.load(Ordering::SeqCst) > 0 {
                break;
            }
            if stop.load(Ordering::SeqCst) {
                log::info!("LocalPlayer::play: cancelled while waiting probe");
                return Err(LocalError::Stream("stopped".into()));
            }
            thread::sleep(Duration::from_millis(20));
        }
        let rate = src_rate.load(Ordering::SeqCst);
        if rate == 0 {
            log::error!("LocalPlayer::play: probe timeout ({OPEN_TIMEOUT:?})");
            self.stop();
            return Err(LocalError::Stream(
                "failed to open audio stream".into(),
            ));
        }
        let channels = src_ch.load(Ordering::SeqCst).max(1) as usize;
        log::info!("LocalPlayer::play: probe ok rate={rate} ch={channels}");

        let cpal_device = pick_cpal_device(device)?;
        let config = pick_output_config(&cpal_device, rate, channels)?;
        let out_rate = config.sample_rate.0;
        let out_ch = config.channels as usize;
        log::info!(
            "LocalPlayer::play: cpal out_rate={out_rate} out_ch={out_ch} (src {rate}/{channels})"
        );

        let ring_cb = Arc::clone(&ring);
        let vol = Arc::clone(&self.volume);
        let stop_cb = Arc::clone(&stop);
        let err_cb = Arc::clone(&err_slot);

        let mut read_pos = 0.0f64;
        let ratio = rate as f64 / out_rate as f64;

        let stream = cpal_device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    if stop_cb.load(Ordering::SeqCst) {
                        data.fill(0.0);
                        return;
                    }
                    let gain = bits_f32(vol.load(Ordering::Relaxed));
                    let mut ring = ring_cb.lock();
                    for frame in data.chunks_mut(out_ch) {
                        let need = ((read_pos.floor() as usize) + 2) * channels;
                        if ring.len() < need {
                            frame.fill(0.0);
                            continue;
                        }
                        let i0 = read_pos.floor() as usize;
                        let frac = (read_pos - i0 as f64) as f32;
                        for c in 0..out_ch {
                            let src_c = c.min(channels - 1);
                            let s0 = ring[i0 * channels + src_c];
                            let s1 = ring[(i0 + 1) * channels + src_c];
                            frame[c] = (s0 + (s1 - s0) * frac) * gain;
                        }
                        read_pos += ratio;
                        let drop_frames = read_pos.floor() as usize;
                        if drop_frames > 0 {
                            let drop_samples = drop_frames * channels;
                            if drop_samples <= ring.len() {
                                ring.drain(..drop_samples);
                                read_pos -= drop_frames as f64;
                            }
                        }
                    }
                },
                move |e| {
                    log::error!("LocalPlayer cpal stream error: {e}");
                    *err_cb.lock() = Some(e.to_string());
                },
                None,
            )
            .map_err(|e| LocalError::Audio(e.to_string()))?;

        stream
            .play()
            .map_err(|e| LocalError::Audio(e.to_string()))?;

        if stop.load(Ordering::SeqCst) {
            log::info!("LocalPlayer::play: cancelled after stream.play");
            drop(stream);
            return Err(LocalError::Stream("stopped".into()));
        }
        self.state.lock().stream = Some(SendStream(stream));

        on_status(&format!("Playing locally: «{}»", device.name));
        thread::sleep(Duration::from_millis(100));
        if stop.load(Ordering::SeqCst) {
            log::info!("LocalPlayer::play: cancelled during settle");
            self.stop();
            return Err(LocalError::Stream("stopped".into()));
        }
        if let Some(e) = err_slot.lock().clone() {
            log::error!("LocalPlayer::play: error after start: {e}");
            self.stop();
            return Err(LocalError::Stream(e));
        }
        log::info!("LocalPlayer::play Ok on '{}'", device.name);
        Ok(())
    }
}

impl Drop for LocalPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn pick_cpal_device(info: &LocalDeviceInfo) -> Result<cpal::Device, LocalError> {
    let host = cpal::default_host();
    if let Some(want) = &info.cpal_name {
        let devices = host
            .output_devices()
            .map_err(|e| LocalError::Audio(e.to_string()))?;
        for d in devices {
            if d.name().ok().as_ref() == Some(want) {
                return Ok(d);
            }
        }
        return Err(LocalError::Audio(format!(
            "device «{want}» not found"
        )));
    }
    host.default_output_device()
        .ok_or_else(|| LocalError::Audio("no default output device".into()))
}

fn pick_output_config(
    device: &cpal::Device,
    _prefer_rate: u32,
    prefer_ch: usize,
) -> Result<cpal::StreamConfig, LocalError> {
    // WASAPI Shared is more reliable with the default mix format; resample in the callback.
    if let Ok(def) = device.default_output_config() {
        return Ok(def.config());
    }

    let supported = device
        .supported_output_configs()
        .map_err(|e| LocalError::Audio(e.to_string()))?;

    let prefer_ch = prefer_ch.clamp(1, 2) as u16;
    let mut best: Option<cpal::SupportedStreamConfigRange> = None;
    for range in supported {
        if range.channels() == prefer_ch {
            best = Some(range);
            break;
        }
        if best.is_none() {
            best = Some(range);
        }
    }
    let range = best.ok_or_else(|| LocalError::Audio("no suitable output format".into()))?;
    Ok(range.with_max_sample_rate().config())
}

fn decode_into_ring(
    url: &str,
    ring: &Mutex<VecDeque<f32>>,
    levels: &Mutex<[f32; BANDS]>,
    stop: &Arc<AtomicBool>,
    src_rate: &AtomicU32,
    src_ch: &AtomicU32,
    ready: &AtomicBool,
    title_tx: Option<&mpsc::Sender<String>>,
) -> Result<(), String> {
    if stop.load(Ordering::SeqCst) {
        return Err("stopped".into());
    }

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "Icy-MetaData",
        reqwest::header::HeaderValue::from_static("1"),
    );
    headers.insert(
        "Accept",
        reqwest::header::HeaderValue::from_static("*/*"),
    );
    headers.insert(
        "User-Agent",
        reqwest::header::HeaderValue::from_static("RockCast/0.1"),
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(None)
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    // `.send()` can hang forever on a dead host (timeout(None) + no headers).
    // Open in a side thread and abandon it on stop/timeout.
    let resp = open_stream_response(client, url, headers, stop)?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    log::info!(
        "LocalPlayer HTTP ok status={} content-type={} icy-metaint={}",
        resp.status(),
        resp.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("?"),
        resp.headers()
            .get("icy-metaint")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
    );

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("audio/mpeg")
        .to_string();
    let meta_int = resp
        .headers()
        .get("icy-metaint")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);

    let body = StopAwareBody::spawn(resp, Arc::clone(stop));
    let reader = IcyStripReader {
        inner: body,
        meta_int,
        until_meta: meta_int,
        stop: Arc::clone(stop),
        title_tx: title_tx.cloned(),
        last_title: String::new(),
    };

    let mss = MediaSourceStream::new(Box::new(reader), Default::default());
    let mut hint = Hint::new();
    apply_hint(&mut hint, &content_type);

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
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
    let sample_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| "missing sample rate".to_string())?;
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(2)
        .max(1);

    src_rate.store(sample_rate, Ordering::SeqCst);
    src_ch.store(channels as u32, Ordering::SeqCst);
    ready.store(true, Ordering::SeqCst);
    log::info!("LocalPlayer decode probe: sample_rate={sample_rate} channels={channels}");

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| e.to_string())?;

    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut pcm_mono = Vec::<f32>::with_capacity(FFT_SIZE * 2);
    let mut fft_buf = vec![Complex::new(0.0, 0.0); FFT_SIZE];
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let window = hann(FFT_SIZE);
    let mut smooth = [0.08f32; BANDS];
    let wall_start = Instant::now();
    let mut samples_done: u64 = 0;

    loop {
        if stop.load(Ordering::SeqCst) {
            return Err("stopped".into());
        }
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

        let interleaved = to_interleaved(&decoded, &mut sample_buf);
        let frames = interleaved.len() / channels.max(1);
        samples_done += frames as u64;

        {
            let mut q = ring.lock();
            // Don't grow the buffer — wait for consumers.
            while q.len() + interleaved.len() > RING_MAX {
                if stop.load(Ordering::SeqCst) {
                    return Err("stopped".into());
                }
                drop(q);
                thread::sleep(Duration::from_millis(5));
                q = ring.lock();
            }
            q.extend(interleaved.iter().copied());
        }

        // Spectrum from mono.
        for frame in interleaved.chunks(channels) {
            let sum: f32 = frame.iter().sum();
            pcm_mono.push(sum / channels as f32);
        }
        while pcm_mono.len() >= FFT_SIZE {
            for i in 0..FFT_SIZE {
                fft_buf[i].re = pcm_mono[i] * window[i];
                fft_buf[i].im = 0.0;
            }
            pcm_mono.drain(..HOP);
            fft.process(&mut fft_buf);
            let bands = magnitudes_to_bands(&fft_buf, sample_rate as f32);
            for (i, b) in bands.iter().enumerate() {
                let rate = if *b > smooth[i] { 0.4 } else { 0.15 };
                smooth[i] += (*b - smooth[i]) * rate;
            }
            *levels.lock() = smooth;
        }

        let audio_secs = samples_done as f64 / f64::from(sample_rate);
        let elapsed = wall_start.elapsed().as_secs_f64();
        if audio_secs > elapsed + 0.25 {
            let ms = ((audio_secs - elapsed - 0.1) * 1000.0).clamp(1.0, 40.0) as u64;
            let until = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < until {
                if stop.load(Ordering::SeqCst) {
                    return Err("stopped".into());
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn open_stream_response(
    client: reqwest::blocking::Client,
    url: &str,
    headers: reqwest::header::HeaderMap,
    stop: &Arc<AtomicBool>,
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
        let wait = deadline.saturating_duration_since(Instant::now()).min(READ_POLL);
        if wait.is_zero() {
            return Err("stream open timeout".into());
        }
        match rx.recv_timeout(wait) {
            Ok(Ok(resp)) => {
                log::debug!("open_stream_response: headers received");
                return Ok(resp);
            }
            Ok(Err(e)) => return Err(e.to_string()),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("failed to open audio stream".into());
            }
        }
    }
}

/// Reads the HTTP body on a side thread so `stop` can interrupt within ~READ_POLL.
struct StopAwareBody {
    rx: Mutex<mpsc::Receiver<io::Result<Vec<u8>>>>,
    stop: Arc<AtomicBool>,
    pending: Vec<u8>,
    pending_at: usize,
}

impl StopAwareBody {
    fn spawn(mut resp: reqwest::blocking::Response, stop: Arc<AtomicBool>) -> Self {
        let (tx, rx) = mpsc::sync_channel(8);
        let stop_prod = Arc::clone(&stop);
        thread::spawn(move || {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                if stop_prod.load(Ordering::SeqCst) {
                    break;
                }
                match resp.read(&mut buf) {
                    Ok(0) => {
                        let _ = tx.send(Ok(Vec::new()));
                        break;
                    }
                    Ok(n) => {
                        if tx.send(Ok(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        break;
                    }
                }
            }
        });
        Self {
            rx: Mutex::new(rx),
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
            let chunk = {
                let rx = self.rx.lock();
                match rx.recv_timeout(READ_POLL) {
                    Ok(Ok(chunk)) if chunk.is_empty() => return Ok(0),
                    Ok(Ok(chunk)) => chunk,
                    Ok(Err(e)) => return Err(e),
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(0),
                }
            };
            self.pending = chunk;
            self.pending_at = 0;
        }
    }
}

fn to_interleaved(
    decoded: &AudioBufferRef<'_>,
    sample_buf: &mut Option<SampleBuffer<f32>>,
) -> Vec<f32> {
    let spec = *decoded.spec();
    let frames = decoded.frames();
    if sample_buf.as_ref().is_none_or(|b| b.capacity() < frames) {
        *sample_buf = Some(SampleBuffer::<f32>::new(frames as u64, spec));
    }
    let buf = sample_buf.as_mut().unwrap();
    buf.copy_interleaved_ref(decoded.clone());
    buf.samples().to_vec()
}

fn apply_hint(hint: &mut Hint, content_type: &str) {
    if content_type.contains("mpeg") || content_type.contains("mp3") {
        hint.with_extension("mp3");
    } else if content_type.contains("aac") || content_type.contains("mp4") {
        hint.with_extension("aac");
    } else if content_type.contains("ogg") || content_type.contains("vorbis") {
        hint.with_extension("ogg");
    } else if content_type.contains("flac") {
        hint.with_extension("flac");
    } else {
        hint.with_extension("mp3");
    }
}

fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let x = std::f32::consts::PI * 2.0 * i as f32 / (n as f32 - 1.0);
            0.5 - 0.5 * x.cos()
        })
        .collect()
}

fn magnitudes_to_bands(fft: &[Complex<f32>], sample_rate: f32) -> [f32; BANDS] {
    let half = FFT_SIZE / 2;
    let f_min = 40.0f32;
    let f_max = (sample_rate * 0.45).min(16_000.0);
    let mut out = [0.0f32; BANDS];
    for b in 0..BANDS {
        let t0 = b as f32 / BANDS as f32;
        let t1 = (b + 1) as f32 / BANDS as f32;
        let lo = f_min * (f_max / f_min).powf(t0);
        let hi = f_min * (f_max / f_min).powf(t1);
        let i0 = ((lo / sample_rate) * FFT_SIZE as f32).floor() as usize;
        let i1 = ((hi / sample_rate) * FFT_SIZE as f32).ceil() as usize;
        let i0 = i0.min(half - 1);
        let i1 = i1.clamp(i0 + 1, half);
        let mut peak = 0.0f32;
        for c in &fft[i0..i1] {
            let m = (c.re * c.re + c.im * c.im).sqrt() / (FFT_SIZE as f32);
            if m > peak {
                peak = m;
            }
        }
        let db = 20.0 * peak.max(1e-8).log10();
        out[b] = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    }
    out
}

struct IcyStripReader {
    inner: StopAwareBody,
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
            if let Some(tx) = &self.title_tx {
                if let Some(title) = parse_stream_title(&meta) {
                    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !title.is_empty() && title != self.last_title {
                        self.last_title = title.clone();
                        let _ = tx.send(title);
                    }
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

fn parse_stream_title(meta: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(meta);
    let lower = text.to_ascii_lowercase();
    let key = "streamtitle='";
    let start = lower.find(key)? + key.len();
    let rest = &text[start..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}
