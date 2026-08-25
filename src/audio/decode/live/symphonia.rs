//! Symphonia fallback decode and prefixed stream source.

use std::{
    io::{self, Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use parking_lot::Mutex;
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
        decode::{codecs, probe},
        format::{StreamFormat, apply_format_hint, infer_stream_format},
        spectrum::SpectrumTap,
    },
    playback_diag,
};

use super::relay::{RelayEmitCtx, relay_emit_pcm};

pub(super) struct SpectrumState {
    tap: SpectrumTap,
}

impl SpectrumState {
    pub(super) fn new(levels: Arc<Mutex<[f32; crate::audio::spectrum::BANDS]>>) -> Self {
        Self {
            tap: SpectrumTap::new(levels),
        }
    }

    pub(super) fn push_pcm(&mut self, pcm: &[f32], channels: usize, sample_rate: u32) {
        self.tap.push_f32(pcm, channels, sample_rate);
    }
}

#[allow(clippy::too_many_arguments)] // PCM/spectrum outputs are separate borrow-checked sinks.
pub(super) fn decode_symphonia_f32(
    url: &str,
    content_type: &str,
    peek: Vec<u8>,
    reader: Box<dyn Read + Send>,
    stop: &Arc<AtomicBool>,
    push_pcm: &mut impl FnMut(&[f32]),
    mut spectrum_state: Option<SpectrumState>,
    src_rate: Arc<std::sync::atomic::AtomicU32>,
    src_ch: Arc<std::sync::atomic::AtomicU32>,
) -> Result<(), String> {
    let stream_format = infer_stream_format(url, content_type, &peek);
    let mut hint = Hint::new();
    apply_format_hint(&mut hint, stream_format);

    let mss = MediaSourceStream::new(
        Box::new(PrefixedMediaSource {
            peek,
            pos: 0,
            inner: std::sync::Mutex::new(reader),
        }),
        Default::default(),
    );

    let probed = select_probe(stream_format)
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

    let mut decoder = codecs::get_codecs()
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
        let spec = *decoded.spec();
        let sample_rate = spec.rate;
        let channels = spec.channels.count().max(1);
        if src_rate.load(Ordering::SeqCst) == 0 {
            src_rate.store(sample_rate, Ordering::SeqCst);
            src_ch.store(channels as u32, Ordering::SeqCst);
        }
        if let Some(state) = spectrum_state.as_mut() {
            state.push_pcm(interleaved, channels, sample_rate);
        }
        playback_diag::decode_pcm(interleaved.len());
        push_pcm(interleaved);
    }
    Err("stopped".into())
}

#[allow(clippy::too_many_arguments)] // Relay state is intentionally passed without heap boxing in the decode loop.
pub(super) fn decode_symphonia_relay(
    url: &str,
    content_type: &str,
    peek: Vec<u8>,
    reader: Box<dyn Read + Send>,
    stop: &Arc<AtomicBool>,
    fanout: &crate::relay::Fanout,
    spectrum: &mut SpectrumTap,
    pcm_bytes: &mut Vec<u8>,
    push: &mut impl FnMut(&[u8]),
    on_format: &mut impl FnMut(u32, u16),
    resampler: &mut crate::audio::decode::pcm::CastPcmResampler,
    smoother: &mut crate::audio::decode::pcm::PcmSmoother,
    format_set: &mut bool,
) -> Result<(), String> {
    let stream_format = infer_stream_format(url, content_type, &peek);
    let mut hint = Hint::new();
    apply_format_hint(&mut hint, stream_format);

    let mss = MediaSourceStream::new(
        Box::new(PrefixedMediaSource {
            peek,
            pos: 0,
            inner: std::sync::Mutex::new(reader),
        }),
        Default::default(),
    );

    let probed = select_probe(stream_format)
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

    let mut decoder = codecs::get_codecs()
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
        if interleaved.is_empty() {
            continue;
        }
        let spec = *decoded.spec();
        let sample_rate = spec.rate;
        let channels = spec.channels.count().max(1) as u16;
        playback_diag::decode_pcm(interleaved.len());
        relay_emit_pcm(
            interleaved,
            sample_rate,
            channels,
            fanout,
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
        fanout.pace_if_full();
    }
    Ok(())
}

fn select_probe(format: StreamFormat) -> &'static symphonia::core::probe::Probe {
    match format {
        StreamFormat::AacAdts => probe::adts_only(),
        _ => symphonia::default::get_probe(),
    }
}

fn next_packet(
    format: &mut dyn symphonia::core::formats::FormatReader,
    stop: &Arc<AtomicBool>,
) -> Result<symphonia::core::formats::Packet, String> {
    if stop.load(Ordering::SeqCst) {
        return Err("stopped".into());
    }
    match format.next_packet() {
        Ok(packet) => Ok(packet),
        Err(SymError::ResetRequired) => Err("reset".into()),
        Err(SymError::IoError(error))
            if error.kind() == io::ErrorKind::UnexpectedEof
                || error.kind() == io::ErrorKind::Interrupted =>
        {
            Err("eof".into())
        }
        Err(_) if stop.load(Ordering::SeqCst) => Err("stopped".into()),
        Err(error) => Err(error.to_string()),
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
