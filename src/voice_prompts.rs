//! Embedded voice prompt playback (beep, "turning on", "not found").

use crate::i18n::Lang;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

static BEEP: &[u8] = include_bytes!("../assets/beep.wav");
static VKLYUCHAYU_RU: &[u8] = include_bytes!("../assets/vklyuchayu_ru.wav");
static NOT_FOUND_RU: &[u8] = include_bytes!("../assets/not_found_ru.wav");
static VKLYUCHAYU_EN: &[u8] = include_bytes!("../assets/vklyuchayu_en.wav");
static NOT_FOUND_EN: &[u8] = include_bytes!("../assets/not_found_en.wav");

#[derive(Clone, Copy)]
pub enum Prompt {
    Beep,
    TurningOn,
    NotFound,
}

pub fn play(prompt: Prompt, lang: Lang) {
    let wav = match prompt {
        Prompt::Beep => BEEP,
        Prompt::TurningOn => match lang {
            Lang::Ru => VKLYUCHAYU_RU,
            Lang::En => VKLYUCHAYU_EN,
        },
        Prompt::NotFound => match lang {
            Lang::Ru => NOT_FOUND_RU,
            Lang::En => NOT_FOUND_EN,
        },
    };
    std::thread::spawn(move || {
        if let Err(e) = play_wav(wav) {
            log::warn!("voice prompt playback failed: {e}");
        }
    });
}

fn play_wav(wav: &[u8]) -> Result<(), String> {
    let (samples, sample_rate, channels) = decode_wav(wav)?;
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("no output device")?;
    let config = cpal::StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };
    let pos = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let pos2 = pos.clone();
    let len = samples.len();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done2 = done.clone();
    let stream = device
        .build_output_stream(
            &config,
            move |out: &mut [f32], _| {
                let start = pos2.load(std::sync::atomic::Ordering::Relaxed);
                for (i, s) in out.iter_mut().enumerate() {
                    let idx = start + i;
                    *s = if idx < len { samples[idx] } else { 0.0 };
                }
                let new_pos = start + out.len();
                pos2.store(new_pos, std::sync::atomic::Ordering::Relaxed);
                if new_pos >= len {
                    done2.store(true, std::sync::atomic::Ordering::Release);
                }
            },
            |e| log::warn!("prompt stream error: {e}"),
            None,
        )
        .map_err(|e| format!("build_output_stream: {e}"))?;
    stream.play().map_err(|e| format!("play: {e}"))?;
    while !done.load(std::sync::atomic::Ordering::Acquire) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Small tail to let the audio buffer flush.
    std::thread::sleep(std::time::Duration::from_millis(50));
    Ok(())
}

fn decode_wav(data: &[u8]) -> Result<(Vec<f32>, u32, u16), String> {
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err("not a WAV".into());
    }
    let channels = u16::from_le_bytes([data[22], data[23]]);
    let sample_rate = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
    let bits = u16::from_le_bytes([data[34], data[35]]);
    // Find "data" chunk.
    let mut off = 12;
    while off + 8 <= data.len() {
        let id = &data[off..off + 4];
        let size = u32::from_le_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]])
            as usize;
        if id == b"data" {
            let pcm = &data[off + 8..data.len().min(off + 8 + size)];
            let samples: Vec<f32> = if bits == 16 {
                pcm.chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                    .collect()
            } else {
                return Err(format!("unsupported {bits}-bit WAV"));
            };
            return Ok((samples, sample_rate, channels));
        }
        off += 8 + size;
        if off % 2 != 0 {
            off += 1;
        }
    }
    Err("WAV data chunk not found".into())
}
