//! Microphone capture for voice commands.

use std::{
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::Duration,
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const MAX_RECORDING: Duration = Duration::from_secs(60);

pub(super) fn record_default_microphone(recording: &AtomicBool) -> Result<(Vec<u8>, u32), String> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or_else(|| "Микрофон Windows не найден".to_owned())?;
    let config = device
        .default_input_config()
        .map_err(|_| "Не удалось прочитать настройки микрофона".to_owned())?;
    let rate = config.sample_rate().0;
    if !matches!(rate, 8_000 | 16_000 | 24_000 | 48_000) {
        return Err(format!(
            "Микрофон использует неподдерживаемую частоту {rate} Hz"
        ));
    }
    let channels = usize::from(config.channels());
    let samples = Arc::new(Mutex::new(Vec::<i16>::new()));
    let out = Arc::clone(&samples);
    let error = |_| log::warn!("microphone capture error");
    let stream = match config.sample_format() {
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config.config(),
            move |data: &[i16], _| push_mono_i16(&out, data, channels),
            error,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config.config(),
            move |data: &[u16], _| {
                let converted: Vec<i16> = data.iter().map(|v| (*v as i32 - 32768) as i16).collect();
                push_mono_i16(&out, &converted, channels)
            },
            error,
            None,
        ),
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.config(),
            move |data: &[f32], _| {
                let converted: Vec<i16> = data
                    .iter()
                    .map(|v| (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                    .collect();
                push_mono_i16(&out, &converted, channels)
            },
            error,
            None,
        ),
        _ => return Err("Формат микрофона не поддерживается".into()),
    }
    .map_err(|_| "Не удалось открыть микрофон".to_owned())?;
    stream
        .play()
        .map_err(|_| "Не удалось начать запись с микрофона".to_owned())?;
    let started = std::time::Instant::now();
    while recording.load(std::sync::atomic::Ordering::Acquire) && started.elapsed() < MAX_RECORDING
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
    let bytes = samples
        .lock()
        .map_err(|_| "Микрофонная запись повреждена".to_owned())?
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    Ok((bytes, rate))
}

fn push_mono_i16(target: &Mutex<Vec<i16>>, input: &[i16], channels: usize) {
    if let Ok(mut target) = target.lock() {
        for frame in input.chunks(channels.max(1)) {
            target.push(
                (frame.iter().map(|v| i32::from(*v)).sum::<i32>() / frame.len().max(1) as i32)
                    as i16,
            );
        }
    }
}
