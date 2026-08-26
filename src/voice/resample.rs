//! PCM16 mono resampling for the public RockServer voice contract (16 kHz).

/// Sample rate required by public `/v1/voice/stream`.
pub(super) const VOICE_SAMPLE_RATE_HZ: u32 = 16_000;

/// Stateful linear resampler for mono PCM16 chunks.
pub(super) struct MonoPcm16Resampler {
    from_hz: u32,
    to_hz: u32,
    src_pos: f64,
    pending: Vec<i16>,
}

impl MonoPcm16Resampler {
    pub(super) fn new(from_hz: u32, to_hz: u32) -> Self {
        Self {
            from_hz: from_hz.max(1),
            to_hz: to_hz.max(1),
            src_pos: 0.0,
            pending: Vec::new(),
        }
    }

    /// Appends `input` and returns newly produced destination-rate samples.
    pub(super) fn push(&mut self, input: &[i16]) -> Vec<i16> {
        if input.is_empty() {
            return Vec::new();
        }
        if self.from_hz == self.to_hz {
            return input.to_vec();
        }
        self.pending.extend_from_slice(input);
        let ratio = f64::from(self.from_hz) / f64::from(self.to_hz);
        let mut out = Vec::new();
        loop {
            let i0 = self.src_pos.floor() as usize;
            if i0 + 1 >= self.pending.len() {
                break;
            }
            let frac = (self.src_pos - i0 as f64) as f32;
            let s0 = f32::from(self.pending[i0]);
            let s1 = f32::from(self.pending[i0 + 1]);
            out.push((s0 + (s1 - s0) * frac).round() as i16);
            self.src_pos += ratio;
            let drop = self.src_pos.floor() as usize;
            if drop > 0 {
                let drop = drop.min(self.pending.len().saturating_sub(1));
                self.pending.drain(..drop);
                self.src_pos -= drop as f64;
            }
        }
        out
    }
}

/// Resamples a complete mono PCM16 buffer to `to_hz`.
pub(super) fn resample_pcm16_mono(input: &[i16], from_hz: u32, to_hz: u32) -> Vec<i16> {
    if input.is_empty() || from_hz == 0 || to_hz == 0 {
        return Vec::new();
    }
    if from_hz == to_hz {
        return input.to_vec();
    }
    let out_len = ((input.len() as u64 * u64::from(to_hz)) / u64::from(from_hz)) as usize;
    if out_len == 0 {
        return Vec::new();
    }
    let last = input.len() - 1;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 * f64::from(from_hz) / f64::from(to_hz);
        let i0 = src_pos.floor() as usize;
        let frac = (src_pos - i0 as f64) as f32;
        let s0 = f32::from(input[i0.min(last)]);
        let s1 = f32::from(input[(i0 + 1).min(last)]);
        out.push((s0 + (s1 - s0) * frac).round() as i16);
    }
    out
}

pub(super) fn pcm16_bytes_to_samples(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

pub(super) fn pcm16_samples_to_bytes(samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        MonoPcm16Resampler, VOICE_SAMPLE_RATE_HZ, pcm16_bytes_to_samples, pcm16_samples_to_bytes,
        resample_pcm16_mono,
    };

    #[test]
    fn voice_contract_rate_is_16k() {
        assert_eq!(VOICE_SAMPLE_RATE_HZ, 16_000);
    }

    #[test]
    fn identity_resample_keeps_samples() {
        let input = vec![1, -2, 3, -4, 5];
        assert_eq!(resample_pcm16_mono(&input, 16_000, 16_000), input);
    }

    #[test]
    fn downsamples_48k_to_16k_with_expected_length() {
        let input: Vec<i16> = (0..480).map(|i| (i * 20) as i16).collect();
        let out = resample_pcm16_mono(&input, 48_000, 16_000);
        assert_eq!(out.len(), 160);
    }

    #[test]
    fn streaming_resampler_matches_batch_length_approx() {
        let input: Vec<i16> = (0..960).map(|i| ((i % 50) * 100) as i16).collect();
        let batch = resample_pcm16_mono(&input, 48_000, 16_000);
        let mut stream = MonoPcm16Resampler::new(48_000, 16_000);
        let mut streamed = Vec::new();
        for chunk in input.chunks(64) {
            streamed.extend(stream.push(chunk));
        }
        assert!((streamed.len() as i32 - batch.len() as i32).abs() <= 2);
    }

    #[test]
    fn pcm16_roundtrip_bytes() {
        let samples = vec![0, -1, 1, i16::MAX, i16::MIN];
        let bytes = pcm16_samples_to_bytes(&samples);
        assert_eq!(pcm16_bytes_to_samples(&bytes), samples);
    }
}
