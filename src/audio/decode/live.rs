//! Unified live internet-radio decode: one path for local speakers and relay.

use std::{
    io::{self, Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
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

use crate::{
    audio::{
        decode::{
            aac::AdtsPcmDecoder,
            icy::{open_stream_response, IcyStreamReader, StopAwareBody},
            opus::LiveOggOpusReader,
            pcm::{CastPcmResampler, PcmSmoother, cast_pcm_rate},
        },
        format::{
            apply_hint, infer_stream_format, read_format_peek, PrefixedReader, StreamFormat,
        },
        spectrum::{SpectrumTap, BANDS},
    },
    net::{metadata_interval, stream_client, stream_headers},
    playback_diag,
};

struct SpectrumState {
    tap: SpectrumTap,
}

impl SpectrumState {
    fn new(levels: Arc<Mutex<[f32; BANDS]>>) -> Self {
        Self {
            tap: SpectrumTap::new(levels),
        }
    }

    fn push_pcm(&mut self, pcm: &[f32], channels: usize, sample_rate: u32) {
        self.tap.push_f32(pcm, channels, sample_rate);
    }
}

struct RelayEmitCtx<'a> {
    format_set: &'a mut bool,
    resampler: &'a mut CastPcmResampler,
    smoother: &'a mut PcmSmoother,
    pcm_bytes: &'a mut Vec<u8>,
}

fn open_icy_reader(
    url: &str,
    stop: &Arc<AtomicBool>,
    title_tx: Option<mpsc::Sender<String>>,
    interruptible_body: bool,
) -> Result<(String, Box<dyn Read + Send>, Vec<u8>), String> {
    let headers = stream_headers(false);
    let client = stream_client(Duration::from_secs(10), None)?;
    let resp = open_stream_response(client, url, headers, stop)?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("audio/mpeg")
        .to_string();
    let meta_int = metadata_interval(resp.headers());
    log::info!("live decode HTTP ok content-type={content_type} icy-metaint={meta_int}");

    let stop_reader = Arc::clone(stop);
    let mut reader: Box<dyn Read + Send> = if interruptible_body {
        Box::new(IcyStreamReader::new(
            StopAwareBody::spawn(resp, Arc::clone(stop)),
            meta_int,
            stop_reader,
            title_tx,
        ))
    } else {
        Box::new(IcyStreamReader::new(
            resp,
            meta_int,
            stop_reader,
            title_tx,
        ))
    };

    let peek = read_format_peek(&mut reader, 8_192, stop)?;
    Ok((content_type, reader, peek))
}

/// Local playback: decode once, push interleaved f32, optional spectrum on same PCM.
pub fn run_live_decode_f32(
    url: &str,
    stop: &Arc<AtomicBool>,
    title_tx: Option<mpsc::Sender<String>>,
    spectrum: Option<Arc<Mutex<[f32; BANDS]>>>,
    src_rate: Arc<AtomicU32>,
    src_ch: Arc<AtomicU32>,
    mut push_pcm: impl FnMut(&[f32]) + Send,
) -> Result<(), String> {
    if stop.load(Ordering::SeqCst) {
        return Err("stopped".into());
    }

    let (content_type, reader, peek) = open_icy_reader(url, stop, title_tx, true)?;
    let format = infer_stream_format(url, &content_type, &peek);
    let mut spectrum_state = spectrum.map(SpectrumState::new);

    let mut push_frame = |pcm: &[f32], rate: u32, ch: u16| -> Result<(), String> {
        if src_rate.load(Ordering::SeqCst) == 0 {
            src_rate.store(rate, Ordering::SeqCst);
            src_ch.store(u32::from(ch.max(1)), Ordering::SeqCst);
        }
        if let Some(state) = spectrum_state.as_mut() {
            state.push_pcm(pcm, ch as usize, rate);
        }
        playback_diag::decode_pcm(pcm.len());
        push_pcm(pcm);
        Ok(())
    };

    match format {
        StreamFormat::AacAdts => {
            let mut decoder = AdtsPcmDecoder::new();
            let mut prefixed = PrefixedReader::new(peek, reader);
            let mut buf = vec![0u8; 16 * 1024];
            let mut last_read = Instant::now();
            while !stop.load(Ordering::SeqCst) {
                let n = prefixed.read(&mut buf).map_err(|e| e.to_string())?;
                playback_diag::http_read(last_read.elapsed(), n);
                last_read = Instant::now();
                if n == 0 {
                    return Err("eof".into());
                }
                let frames = decoder.push_f32(&buf[..n]).map_err(|e| e.to_string())?;
                if frames.is_empty() {
                    continue;
                }
                let rate = decoder.sample_rate().unwrap_or(44_100);
                let ch = decoder.channels().unwrap_or(2).max(1);
                for frame in &frames {
                    push_frame(frame, rate, ch)?;
                }
            }
        }
        StreamFormat::OpusOgg => {
            src_rate.store(48_000, Ordering::SeqCst);
            src_ch.store(2, Ordering::SeqCst);
            let mut reader = LiveOggOpusReader::new(PrefixedReader::new(peek, reader));
            let mut decoder = OpusDecoder::new(48_000, OpusChannels::Stereo)
                .map_err(|e| format!("opus: {e}"))?;
            let mut pcm = vec![0i16; 5760 * 2];
            while !stop.load(Ordering::SeqCst) {
                let packet = match reader.read_packet(stop)? {
                    Some(p) => p,
                    None => return Err("eof".into()),
                };
                let samples = decoder.decode(&packet, &mut pcm, DecodeMode::Normal).unwrap_or(0);
                if samples == 0 {
                    continue;
                }
                let pcm_f32: Vec<f32> = pcm[..samples * 2]
                    .iter()
                    .map(|s| f32::from(*s) / f32::from(i16::MAX))
                    .collect();
                push_frame(&pcm_f32, 48_000, 2)?;
            }
        }
        _ => {
            decode_symphonia_f32(
                url,
                &content_type,
                peek,
                reader,
                stop,
                &mut push_pcm,
                spectrum_state,
                src_rate,
                src_ch,
            )?;
        }
    }
    Err("stopped".into())
}

fn relay_emit_pcm(
    pcm: &[f32],
    rate: u32,
    ch: u16,
    ctx: RelayEmitCtx<'_>,
    spectrum: &mut SpectrumTap,
    on_format: &mut impl FnMut(u32, u16),
    push: &mut impl FnMut(&[u8]),
) {
    if !*ctx.format_set {
        ctx.resampler.set_format(rate, ch, cast_pcm_rate());
        ctx.smoother.set_format(cast_pcm_rate(), ch);
        on_format(cast_pcm_rate(), ch);
        *ctx.format_set = true;
    }
    let out_rate = cast_pcm_rate();
    let tap = spectrum as *mut SpectrumTap;
    let ch_usize = ch as usize;
    ctx.resampler.push(pcm, |resampled| {
        ctx.pcm_bytes.clear();
        ctx.pcm_bytes.reserve(resampled.len() * 2);
        for sample in resampled {
            let v = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
            ctx.pcm_bytes.extend_from_slice(&v.to_le_bytes());
        }
        ctx.smoother.push(ctx.pcm_bytes, |chunk| {
            // SAFETY: single decode thread; spectrum on fixed 20 ms relay frames.
            unsafe {
                (*tap).push_i16_le(chunk, ch_usize, out_rate);
            }
            push(chunk);
        });
    });
}

/// Relay transcode: decode once, push smoothed 16-bit PCM; spectrum at 48 kHz.
pub fn run_live_decode_relay_pcm(
    url: &str,
    stop: &Arc<AtomicBool>,
    spectrum: &mut SpectrumTap,
    mut push: impl FnMut(&[u8]) + Send,
    mut on_format: impl FnMut(u32, u16) + Send,
) -> Result<(), String> {
    if stop.load(Ordering::SeqCst) {
        return Err("stopped".into());
    }

    let (content_type, reader, peek) = open_icy_reader(url, stop, None, true)?;
    let format = infer_stream_format(url, &content_type, &peek);
    let mut resampler = CastPcmResampler::new(2);
    let mut smoother = PcmSmoother::new(cast_pcm_rate(), 2);
    let mut format_set = false;
    let mut pcm_bytes = Vec::with_capacity(8192);

    match format {
        StreamFormat::AacAdts => {
            let mut decoder = AdtsPcmDecoder::new();
            let mut prefixed = PrefixedReader::new(peek, reader);
            let mut buf = vec![0u8; 16 * 1024];
            let mut last_read = Instant::now();
            while !stop.load(Ordering::SeqCst) {
                let n = prefixed.read(&mut buf).map_err(|e| e.to_string())?;
                playback_diag::http_read(last_read.elapsed(), n);
                last_read = Instant::now();
                if n == 0 {
                    return Err("eof".into());
                }
                let frames = decoder.push_f32(&buf[..n]).map_err(|e| e.to_string())?;
                if frames.is_empty() {
                    continue;
                }
                let rate = decoder.sample_rate().unwrap_or(44_100);
                let ch = decoder.channels().unwrap_or(2).max(1);
                for frame in &frames {
                    relay_emit_pcm(
                        frame,
                        rate,
                        ch,
                        RelayEmitCtx {
                            format_set: &mut format_set,
                            resampler: &mut resampler,
                            smoother: &mut smoother,
                            pcm_bytes: &mut pcm_bytes,
                        },
                        spectrum,
                        &mut on_format,
                        &mut push,
                    );
                }
            }
        }
        StreamFormat::OpusOgg => {
            let mut reader = LiveOggOpusReader::new(PrefixedReader::new(peek, reader));
            let mut decoder = OpusDecoder::new(48_000, OpusChannels::Stereo)
                .map_err(|e| format!("opus: {e}"))?;
            let mut pcm = vec![0i16; 5760 * 2];
            while !stop.load(Ordering::SeqCst) {
                let packet = match reader.read_packet(stop)? {
                    Some(p) => p,
                    None => return Err("eof".into()),
                };
                let samples = decoder.decode(&packet, &mut pcm, DecodeMode::Normal).unwrap_or(0);
                if samples == 0 {
                    continue;
                }
                let pcm_f32: Vec<f32> = pcm[..samples * 2]
                    .iter()
                    .map(|s| f32::from(*s) / f32::from(i16::MAX))
                    .collect();
                relay_emit_pcm(
                    &pcm_f32,
                    48_000,
                    2,
                    RelayEmitCtx {
                        format_set: &mut format_set,
                        resampler: &mut resampler,
                        smoother: &mut smoother,
                        pcm_bytes: &mut pcm_bytes,
                    },
                    spectrum,
                    &mut on_format,
                    &mut push,
                );
            }
        }
        _ => {
            decode_symphonia_relay(
                url,
                &content_type,
                peek,
                reader,
                stop,
                spectrum,
                &mut pcm_bytes,
                &mut push,
                &mut on_format,
                &mut resampler,
                &mut smoother,
                &mut format_set,
            )?;
        }
    }
    smoother.flush(|chunk| push(chunk));
    Err("stopped".into())
}

fn decode_symphonia_f32(
    url: &str,
    content_type: &str,
    peek: Vec<u8>,
    reader: Box<dyn Read + Send>,
    stop: &Arc<AtomicBool>,
    push_pcm: &mut impl FnMut(&[f32]),
    mut spectrum_state: Option<SpectrumState>,
    src_rate: Arc<AtomicU32>,
    src_ch: Arc<AtomicU32>,
) -> Result<(), String> {
    let mss = MediaSourceStream::new(
        Box::new(PrefixedMediaSource {
            peek,
            pos: 0,
            inner: std::sync::Mutex::new(reader),
        }),
        Default::default(),
    );
    let mut hint = Hint::new();
    apply_hint(&mut hint, url, content_type, &[]);

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
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44_100);
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(2)
        .max(1);
    src_rate.store(sample_rate, Ordering::SeqCst);
    src_ch.store(channels as u32, Ordering::SeqCst);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| e.to_string())?;

    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    while !stop.load(Ordering::SeqCst) {
        let packet = next_packet(format.as_mut(), stop)?;
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(e.to_string()),
        };
        let interleaved = copy_interleaved(&decoded, &mut sample_buf);
        if let Some(state) = spectrum_state.as_mut() {
            state.push_pcm(interleaved, channels, sample_rate);
        }
        push_pcm(interleaved);
    }
    Err("stopped".into())
}

fn decode_symphonia_relay(
    url: &str,
    content_type: &str,
    peek: Vec<u8>,
    reader: Box<dyn Read + Send>,
    stop: &Arc<AtomicBool>,
    spectrum: &mut SpectrumTap,
    pcm_bytes: &mut Vec<u8>,
    push: &mut impl FnMut(&[u8]),
    on_format: &mut impl FnMut(u32, u16),
    resampler: &mut CastPcmResampler,
    smoother: &mut PcmSmoother,
    format_set: &mut bool,
) -> Result<(), String> {
    let mss = MediaSourceStream::new(
        Box::new(PrefixedMediaSource {
            peek,
            pos: 0,
            inner: std::sync::Mutex::new(reader),
        }),
        Default::default(),
    );
    let mut hint = Hint::new();
    apply_hint(&mut hint, url, content_type, &[]);

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
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44_100);
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(2)
        .max(1) as u16;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| e.to_string())?;

    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    while !stop.load(Ordering::SeqCst) {
        let packet = next_packet(format.as_mut(), stop)?;
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(e.to_string()),
        };
        let interleaved = copy_interleaved(&decoded, &mut sample_buf);
        relay_emit_pcm(
            interleaved,
            sample_rate,
            channels,
            RelayEmitCtx {
                format_set,
                resampler,
                smoother,
                pcm_bytes,
            },
            spectrum,
            on_format,
            push,
        );
    }
    Ok(())
}

fn next_packet(
    format: &mut dyn symphonia::core::formats::FormatReader,
    stop: &Arc<AtomicBool>,
) -> Result<symphonia::core::formats::Packet, String> {
    loop {
        match format.next_packet() {
            Ok(p) => return Ok(p),
            Err(SymError::ResetRequired) => return Err("reset".into()),
            Err(SymError::IoError(e))
                if e.kind() == io::ErrorKind::UnexpectedEof
                    || e.kind() == io::ErrorKind::Interrupted =>
            {
                return Err("eof".into());
            }
            Err(_) if stop.load(Ordering::SeqCst) => return Err("stopped".into()),
            Err(e) => return Err(e.to_string()),
        }
    }
}

struct PrefixedMediaSource {
    peek: Vec<u8>,
    pos: usize,
    inner: std::sync::Mutex<Box<dyn Read + Send>>,
}

impl Read for PrefixedMediaSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos < self.peek.len() {
            let n = buf.len().min(self.peek.len() - self.pos);
            buf[..n].copy_from_slice(&self.peek[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        self.inner.lock().unwrap().read(buf)
    }
}

impl Seek for PrefixedMediaSource {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "stream is not seekable",
        ))
    }
}

impl MediaSource for PrefixedMediaSource {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

fn copy_interleaved<'a>(
    decoded: &AudioBufferRef<'_>,
    sample_buf: &'a mut Option<SampleBuffer<f32>>,
) -> &'a [f32] {
    let spec = *decoded.spec();
    let frames = decoded.frames();
    if sample_buf.as_ref().is_none_or(|b| b.capacity() < frames) {
        *sample_buf = Some(SampleBuffer::<f32>::new(frames as u64, spec));
    }
    let buf = sample_buf.as_mut().unwrap();
    buf.copy_interleaved_ref(decoded.clone());
    buf.samples()
}
