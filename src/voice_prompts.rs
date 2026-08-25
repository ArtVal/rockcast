//! Embedded voice prompt playback (beep, "turning on", "not found").

use crate::i18n::Lang;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

static BEEP: &[u8] = include_bytes!("../assets/beep.wav");
static VKLYUCHAYU_RU: &[u8] = include_bytes!("../assets/vklyuchayu_ru.wav");
static NOT_FOUND_RU: &[u8] = include_bytes!("../assets/not_found_ru.wav");
static VKLYUCHAYU_EN: &[u8] = include_bytes!("../assets/vklyuchayu_en.wav");
static NOT_FOUND_EN: &[u8] = include_bytes!("../assets/not_found_en.wav");
static SERVER_UNAVAILABLE_RU: &[u8] = include_bytes!("../assets/server_unavailable_ru.wav");
static SERVER_UNAVAILABLE_EN: &[u8] = include_bytes!("../assets/server_unavailable_en.wav");
static TOKEN_MISSING_RU: &[u8] = include_bytes!("../assets/token_missing_ru.wav");
static TOKEN_MISSING_EN: &[u8] = include_bytes!("../assets/token_missing_en.wav");
static TOKEN_INVALID_RU: &[u8] = include_bytes!("../assets/token_invalid_ru.wav");
static TOKEN_INVALID_EN: &[u8] = include_bytes!("../assets/token_invalid_en.wav");
static STATION_UNAVAILABLE_RU: &[u8] = include_bytes!("../assets/station_unavailable_ru.wav");
static STATION_UNAVAILABLE_EN: &[u8] = include_bytes!("../assets/station_unavailable_en.wav");

#[derive(Clone, Copy)]
pub enum Prompt {
    Beep,
    TurningOn,
    NotFound,
    ServerUnavailable,
    TokenMissing,
    TokenInvalid,
    StationUnavailable,
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
        Prompt::ServerUnavailable => match lang {
            Lang::Ru => SERVER_UNAVAILABLE_RU,
            Lang::En => SERVER_UNAVAILABLE_EN,
        },
        Prompt::TokenMissing => match lang {
            Lang::Ru => TOKEN_MISSING_RU,
            Lang::En => TOKEN_MISSING_EN,
        },
        Prompt::TokenInvalid => match lang {
            Lang::Ru => TOKEN_INVALID_RU,
            Lang::En => TOKEN_INVALID_EN,
        },
        Prompt::StationUnavailable => match lang {
            Lang::Ru => STATION_UNAVAILABLE_RU,
            Lang::En => STATION_UNAVAILABLE_EN,
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
    let supported = device
        .default_output_config()
        .map_err(|e| format!("default output configuration: {e}"))?;
    let config = supported.config();
    let output_channels = usize::from(config.channels);
    let output_rate = config.sample_rate.0;
    let pos = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let pos2 = pos.clone();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done2 = done.clone();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config,
            move |out: &mut [f32], _| {
                fill_prompt(
                    out,
                    &samples,
                    sample_rate,
                    channels,
                    output_rate,
                    output_channels,
                    &pos2,
                    &done2,
                );
            },
            |e| log::warn!("prompt stream error: {e}"),
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            &config,
            move |out: &mut [i16], _| {
                fill_prompt_i16(
                    out,
                    &samples,
                    sample_rate,
                    channels,
                    output_rate,
                    output_channels,
                    &pos2,
                    &done2,
                );
            },
            |e| log::warn!("prompt stream error: {e}"),
            None,
        ),
        cpal::SampleFormat::U16 => device.build_output_stream(
            &config,
            move |out: &mut [u16], _| {
                fill_prompt_u16(
                    out,
                    &samples,
                    sample_rate,
                    channels,
                    output_rate,
                    output_channels,
                    &pos2,
                    &done2,
                );
            },
            |e| log::warn!("prompt stream error: {e}"),
            None,
        ),
        format => return Err(format!("unsupported output sample format {format:?}")),
    }
    .map_err(|e| format!("build_output_stream: {e}"))?;
    stream.play().map_err(|e| format!("play: {e}"))?;
    while !done.load(std::sync::atomic::Ordering::Acquire) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Small tail to let the audio buffer flush.
    std::thread::sleep(std::time::Duration::from_millis(50));
    Ok(())
}

#[allow(clippy::too_many_arguments)] // Audio callback format parameters are supplied separately by cpal.
fn prompt_samples(
    output_len: usize,
    samples: &[f32],
    source_rate: u32,
    source_channels: u16,
    output_rate: u32,
    output_channels: usize,
    position: &std::sync::atomic::AtomicUsize,
    done: &std::sync::atomic::AtomicBool,
) -> Vec<f32> {
    let start_frame = position.load(std::sync::atomic::Ordering::Relaxed);
    let source_channels = usize::from(source_channels).max(1);
    let source_frames = samples.len().div_ceil(source_channels);
    let frames = output_len.div_ceil(output_channels.max(1));
    let mut output = vec![0.0; output_len];
    for frame in 0..frames {
        let source_frame =
            start_frame + (frame * source_rate as usize / output_rate.max(1) as usize);
        if source_frame >= source_frames {
            continue;
        }
        for channel in 0..output_channels {
            let source_channel = channel.min(source_channels - 1);
            if let Some(sample) = output.get_mut(frame * output_channels + channel) {
                *sample = samples[source_frame * source_channels + source_channel];
            }
        }
    }
    let advanced = frames * source_rate as usize / output_rate.max(1) as usize;
    let next_frame = start_frame + advanced.max(1);
    position.store(next_frame, std::sync::atomic::Ordering::Relaxed);
    if next_frame >= source_frames {
        done.store(true, std::sync::atomic::Ordering::Release);
    }
    output
}

#[allow(clippy::too_many_arguments)]
fn fill_prompt(
    out: &mut [f32],
    samples: &[f32],
    source_rate: u32,
    source_channels: u16,
    output_rate: u32,
    output_channels: usize,
    position: &std::sync::atomic::AtomicUsize,
    done: &std::sync::atomic::AtomicBool,
) {
    out.copy_from_slice(&prompt_samples(
        out.len(),
        samples,
        source_rate,
        source_channels,
        output_rate,
        output_channels,
        position,
        done,
    ));
}

#[allow(clippy::too_many_arguments)]
fn fill_prompt_i16(
    out: &mut [i16],
    samples: &[f32],
    source_rate: u32,
    source_channels: u16,
    output_rate: u32,
    output_channels: usize,
    position: &std::sync::atomic::AtomicUsize,
    done: &std::sync::atomic::AtomicBool,
) {
    let rendered = prompt_samples(
        out.len(),
        samples,
        source_rate,
        source_channels,
        output_rate,
        output_channels,
        position,
        done,
    );
    for (target, sample) in out.iter_mut().zip(rendered) {
        *target = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_prompt_u16(
    out: &mut [u16],
    samples: &[f32],
    source_rate: u32,
    source_channels: u16,
    output_rate: u32,
    output_channels: usize,
    position: &std::sync::atomic::AtomicUsize,
    done: &std::sync::atomic::AtomicBool,
) {
    let rendered = prompt_samples(
        out.len(),
        samples,
        source_rate,
        source_channels,
        output_rate,
        output_channels,
        position,
        done,
    );
    for (target, sample) in out.iter_mut().zip(rendered) {
        *target = ((sample.clamp(-1.0, 1.0) + 1.0) * 32767.5) as u16;
    }
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
