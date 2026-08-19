//! Relay transcode: decode once, push smoothed 16-bit PCM for Cast feeder.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use ropus::{Channels as OpusChannels, DecodeMode, Decoder as OpusDecoder};

use crate::audio::{
    decode::{
        opus::LiveOggOpusReader,
        pcm::{CastPcmResampler, PcmSmoother, cast_pcm_rate},
    },
    format::{infer_stream_format, PrefixedReader, StreamFormat},
    spectrum::SpectrumTap,
};

use super::{open::open_icy_reader, adts::decode_fdk_adts_relay, symphonia::decode_symphonia_relay};

pub(super) struct RelayEmitCtx<'a> {
    pub format_set: &'a mut bool,
    pub resampler: &'a mut CastPcmResampler,
    pub smoother: &'a mut PcmSmoother,
    pub pcm_bytes: &'a mut Vec<u8>,
}

pub(super) fn relay_emit_pcm(
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
    let ch_usize = ch as usize;
    ctx.resampler.push(pcm, |resampled| {
        spectrum.push_f32(resampled, ch_usize, out_rate);
        ctx.pcm_bytes.clear();
        ctx.pcm_bytes.reserve(resampled.len() * 2);
        for sample in resampled {
            let v = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
            ctx.pcm_bytes.extend_from_slice(&v.to_le_bytes());
        }
        ctx.smoother.push(ctx.pcm_bytes, |chunk| {
            push(chunk);
        });
    });
}

/// Relay transcode: decode once, push smoothed 16-bit PCM; spectrum at 48 kHz.
pub fn run_live_decode_relay_pcm(
    url: &str,
    stop: &Arc<AtomicBool>,
    fanout: &crate::relay::Fanout,
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
                fanout.pace_if_full();
            }
        }
        StreamFormat::AacAdts => {
            decode_fdk_adts_relay(
                peek,
                reader,
                stop,
                fanout,
                spectrum,
                &mut pcm_bytes,
                &mut push,
                &mut on_format,
                &mut resampler,
                &mut smoother,
                &mut format_set,
            )?;
        }
        _ => {
            decode_symphonia_relay(
                url,
                &content_type,
                peek,
                reader,
                stop,
                fanout,
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
