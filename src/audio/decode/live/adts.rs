//! Live ADTS AAC via libfdk-aac — supports multi-frame ADTS (e.g. SomaFM).

use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use fdk_aac::dec::{Decoder, DecoderError, Transport};

use crate::{
    audio::format::{find_adts_sync, PrefixedReader},
    playback_diag,
};

use super::relay::{relay_emit_pcm, RelayEmitCtx};
use super::symphonia::SpectrumState;

const MAX_PCM_SAMPLES: usize = 8192 * 2;

/// PCM output rate after SBR/PS upsampling (`sampleRate`), not core AAC rate (`aacSampleRate`).
fn fdk_pcm_sample_rate(info: &fdk_aac::dec::StreamInfo) -> u32 {
    if info.sampleRate > 0 {
        info.sampleRate as u32
    } else {
        info.aacSampleRate.max(1) as u32
    }
}

fn adts_total_frame_len(data: &[u8]) -> Option<usize> {
    if data.len() < 7 || data[0] != 0xFF || (data[1] & 0xF6) != 0xF0 {
        return None;
    }
    let frame_len =
        ((data[3] as usize & 0x03) << 11) | ((data[4] as usize) << 3) | ((data[5] as usize & 0xE0) >> 5);
    (frame_len >= 7).then_some(frame_len)
}

struct AdtsStreamReader<R: Read> {
    inner: R,
    pending: Vec<u8>,
}

impl<R: Read> AdtsStreamReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            pending: Vec::with_capacity(16_384),
        }
    }

    fn refill(&mut self, stop: &AtomicBool) -> Result<bool, String> {
        if stop.load(Ordering::SeqCst) {
            return Err("stopped".into());
        }
        let mut chunk = [0u8; 4096];
        match self.inner.read(&mut chunk) {
            Ok(0) => Ok(false),
            Ok(n) => {
                self.pending.extend_from_slice(&chunk[..n]);
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Ok(true),
            Err(e) => Err(e.to_string()),
        }
    }

    fn next_frame(&mut self, stop: &AtomicBool) -> Result<Option<Vec<u8>>, String> {
        loop {
            if stop.load(Ordering::SeqCst) {
                return Err("stopped".into());
            }
            if let Some(sync) = find_adts_sync(&self.pending) {
                if sync > 0 {
                    self.pending.drain(..sync);
                }
                if let Some(len) = adts_total_frame_len(&self.pending) {
                    if self.pending.len() >= len {
                        return Ok(Some(self.pending.drain(..len).collect()));
                    }
                }
            } else if self.pending.len() > 32_768 {
                self.pending.clear();
            }
            if !self.refill(stop)? {
                return Ok(None);
            }
        }
    }
}

fn pcm_i16_to_f32(pcm: &[i16]) -> Vec<f32> {
    pcm.iter()
        .map(|&s| f32::from(s) / f32::from(i16::MAX))
        .collect()
}

fn emit_fdk_pcm(
    decoder: &Decoder,
    pcm_i16: &[i16],
    push_pcm: &mut impl FnMut(&[f32]),
    spectrum_state: &mut Option<SpectrumState>,
    src_rate: &Arc<std::sync::atomic::AtomicU32>,
    src_ch: &Arc<std::sync::atomic::AtomicU32>,
) {
    let info = decoder.stream_info();
    let channels = info.numChannels.max(1) as usize;
    let sample_rate = fdk_pcm_sample_rate(info);
    let size = decoder.decoded_frame_size();
    if size == 0 || pcm_i16.len() < size {
        return;
    }
    let pcm_f32 = pcm_i16_to_f32(&pcm_i16[..size]);
    if src_rate.load(Ordering::SeqCst) == 0 {
        src_rate.store(sample_rate, Ordering::SeqCst);
        src_ch.store(channels as u32, Ordering::SeqCst);
    }
    if let Some(state) = spectrum_state.as_mut() {
        state.push_pcm(&pcm_f32, channels, sample_rate);
    }
    playback_diag::decode_pcm(pcm_f32.len());
    push_pcm(&pcm_f32);
}


pub(super) fn decode_fdk_adts_f32(
    peek: Vec<u8>,
    reader: Box<dyn Read + Send>,
    stop: &Arc<AtomicBool>,
    push_pcm: &mut impl FnMut(&[f32]),
    mut spectrum_state: Option<SpectrumState>,
    src_rate: Arc<std::sync::atomic::AtomicU32>,
    src_ch: Arc<std::sync::atomic::AtomicU32>,
) -> Result<(), String> {
    let mut stream = AdtsStreamReader::new(PrefixedReader::new(peek, reader));
    let mut decoder = Decoder::new(Transport::Adts);
    let mut pcm_i16 = vec![0i16; MAX_PCM_SAMPLES];

    loop {
        let frame = match stream.next_frame(stop)? {
            Some(f) => f,
            None => return Err("eof".into()),
        };
        if decoder.fill(&frame).is_err() {
            continue;
        }
        loop {
            match decoder.decode_frame(&mut pcm_i16) {
                Ok(()) => emit_fdk_pcm(
                    &decoder,
                    &pcm_i16,
                    push_pcm,
                    &mut spectrum_state,
                    &src_rate,
                    &src_ch,
                ),
                Err(DecoderError::NOT_ENOUGH_BITS) | Err(DecoderError::TRANSPORT_SYNC_ERROR) => break,
                Err(e) => {
                    log::debug!("fdk-aac frame skip: {}", e.message());
                    break;
                }
            }
        }
    }
}

pub(super) fn decode_fdk_adts_relay(
    peek: Vec<u8>,
    reader: Box<dyn Read + Send>,
    stop: &Arc<AtomicBool>,
    fanout: &crate::relay::Fanout,
    spectrum: &mut crate::audio::spectrum::SpectrumTap,
    pcm_bytes: &mut Vec<u8>,
    push: &mut impl FnMut(&[u8]),
    on_format: &mut impl FnMut(u32, u16),
    resampler: &mut crate::audio::decode::pcm::CastPcmResampler,
    smoother: &mut crate::audio::decode::pcm::PcmSmoother,
    format_set: &mut bool,
) -> Result<(), String> {
    let mut stream = AdtsStreamReader::new(PrefixedReader::new(peek, reader));
    let mut decoder = Decoder::new(Transport::Adts);
    let mut pcm_i16 = vec![0i16; MAX_PCM_SAMPLES];

    loop {
        let frame = match stream.next_frame(stop)? {
            Some(f) => f,
            None => return Err("eof".into()),
        };
        if decoder.fill(&frame).is_err() {
            continue;
        }
        loop {
            match decoder.decode_frame(&mut pcm_i16) {
                Ok(()) => {
                    let info = decoder.stream_info();
                    let channels = info.numChannels.max(1) as u16;
                    let sample_rate = fdk_pcm_sample_rate(info);
                    let size = decoder.decoded_frame_size();
                    if size == 0 || pcm_i16.len() < size {
                        continue;
                    }
                    let pcm_f32 = pcm_i16_to_f32(&pcm_i16[..size]);
                    playback_diag::decode_pcm(pcm_f32.len());
                    relay_emit_pcm(
                        &pcm_f32,
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
                    fanout.pace_if_full();
                }
                Err(DecoderError::NOT_ENOUGH_BITS) | Err(DecoderError::TRANSPORT_SYNC_ERROR) => break,
                Err(e) => {
                    log::debug!("fdk-aac frame skip: {}", e.message());
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_somafm_first_frame_length() {
        let data = [
            0xFF, 0xF9, 0x5C, 0x80, 0x5C, 0xE1, 0x8C, 0x21, 0x1B, 0x55,
        ];
        let len = adts_total_frame_len(&data).expect("adts len");
        assert!(len >= 7 && len < 2048, "unexpected frame len {len}");
    }
}
