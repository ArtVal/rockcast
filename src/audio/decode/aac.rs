//! Incremental ADTS (AAC/AAC+) decode to interleaved PCM.

use fdk_aac_rust::{adts::AdtsHeader, transport::PureRustTransportDecoder};

use crate::audio::format::find_adts_sync;

pub struct AdtsPcmDecoder {
    decoder: Option<PureRustTransportDecoder>,
    pending: Vec<u8>,
}

impl Default for AdtsPcmDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl AdtsPcmDecoder {
    pub fn new() -> Self {
        Self {
            decoder: None,
            pending: Vec::new(),
        }
    }

    pub fn sample_rate(&self) -> Option<u32> {
        self.decoder
            .as_ref()
            .map(|decoder| decoder.stream_info().sample_rate)
    }

    pub fn channels(&self) -> Option<u16> {
        self.decoder
            .as_ref()
            .map(|decoder| decoder.stream_info().num_channels as u16)
    }

    /// Push raw stream bytes; returns newly decoded interleaved PCM (f32, nominal ±1.0).
    pub fn push_f32(&mut self, data: &[u8]) -> Result<Vec<Vec<f32>>, String> {
        if data.is_empty() {
            return Ok(Vec::new());
        }
        self.pending.extend_from_slice(data);
        self.try_init()?;
        let Some(decoder) = self.decoder.as_mut() else {
            trim_pending_without_sync(&mut self.pending);
            return Ok(Vec::new());
        };
        if !self.pending.is_empty() {
            let chunk = std::mem::take(&mut self.pending);
            decoder
                .push_adts_bytes(&chunk)
                .map_err(|error| error.to_string())?;
        }
        decoder
            .drain_adts_interleaved_f32()
            .map_err(|error| error.to_string())
    }

    pub fn push(&mut self, data: &[u8]) -> Result<Vec<Vec<i16>>, String> {
        Ok(self
            .push_f32(data)?
            .into_iter()
            .map(|frame| {
                frame
                    .iter()
                    .map(|sample| (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)
                    .collect()
            })
            .collect())
    }

    fn try_init(&mut self) -> Result<(), String> {
        if self.decoder.is_some() {
            return Ok(());
        }
        if let Some(sync) = find_adts_sync(&self.pending) {
            if sync > 0 {
                self.pending.drain(..sync);
            }
        }
        let header = match AdtsHeader::parse(&self.pending) {
            Ok(header) => header,
            Err(_) => return Ok(()),
        };
        if self.pending.len() < header.frame_length {
            return Ok(());
        }
        self.decoder = Some(
            PureRustTransportDecoder::from_adts_frame(&self.pending[..header.frame_length])
                .map_err(|error| error.to_string())?,
        );
        Ok(())
    }
}

fn trim_pending_without_sync(pending: &mut Vec<u8>) {
    const MAX_PENDING: usize = 256 * 1024;
    if pending.len() <= MAX_PENDING {
        return;
    }
    if let Some(sync) = find_adts_sync(pending) {
        pending.drain(..sync);
    } else {
        pending.clear();
    }
}

pub fn pcm_frames_to_bytes(frames: &[Vec<i16>]) -> Vec<u8> {
    let samples = frames.iter().map(Vec::len).sum::<usize>();
    let mut out = Vec::with_capacity(samples * 2);
    for frame in frames {
        for sample in frame {
            out.extend_from_slice(&sample.to_le_bytes());
        }
    }
    out
}

pub fn pcm_frames_to_f32(frames: &[Vec<f32>]) -> Vec<f32> {
    frames.iter().flat_map(|frame| frame.iter().copied()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdk_aac_rust::aac_encoder::PureRustAacLcMonoEncoder;

    #[test]
    fn decodes_generated_adts_stream() {
        let mut encoder = PureRustAacLcMonoEncoder::new(4, 44_100, 64_000).unwrap();
        let pcm_in: Vec<f32> = (0..1024)
            .map(|sample| (sample as f32 * 0.01).sin() * 0.2)
            .collect();
        let frame = encoder.encode_adts_frame(&pcm_in).unwrap();

        let mut decoder = AdtsPcmDecoder::new();
        let decoded = decoder.push_f32(&frame).unwrap();
        assert!(!decoded.is_empty());
        assert_eq!(decoder.sample_rate(), Some(44_100));
    }
}
