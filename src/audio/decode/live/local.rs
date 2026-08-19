//! Local f32 decode path (speakers + optional spectrum tap).

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc,
};

use parking_lot::Mutex;
use ropus::{Channels as OpusChannels, DecodeMode, Decoder as OpusDecoder};

use crate::audio::{
    decode::opus::LiveOggOpusReader,
    format::{infer_stream_format, PrefixedReader, StreamFormat},
    spectrum::BANDS,
};

use super::{open::open_icy_reader, adts::{decode_fdk_adts_f32}, symphonia::{decode_symphonia_f32, SpectrumState}};

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
        push_pcm(pcm);
        Ok(())
    };

    match format {
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
        StreamFormat::AacAdts => {
            decode_fdk_adts_f32(
                peek,
                reader,
                stop,
                &mut push_pcm,
                spectrum_state,
                src_rate,
                src_ch,
            )?;
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
