//! Microphone capture for voice commands.

use std::{
    sync::{Arc, Mutex, atomic::AtomicBool, mpsc},
    time::Duration,
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const MAX_RECORDING: Duration = Duration::from_secs(60);

pub(super) fn record_default_microphone(recording: &AtomicBool) -> Result<(Vec<u8>, u32), String> {
    let samples = Arc::new(Mutex::new(Vec::<i16>::new()));
    let output = Arc::clone(&samples);
    let sample_rate = record_microphone(recording, move |chunk| {
        if let Ok(mut output) = output.lock() {
            output.extend_from_slice(&chunk);
        }
        Ok(())
    })?;
    let bytes = samples
        .lock()
        .map_err(|_| "Микрофонная запись повреждена".to_owned())?
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    Ok((bytes, sample_rate))
}

/// Captures microphone audio and invokes the consumer with short PCM16 mono chunks while recording.
pub(super) fn stream_default_microphone(
    recording: &AtomicBool,
    mut consume: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<u32, String> {
    record_microphone(recording, move |chunk| {
        let bytes = chunk
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        consume(&bytes)
    })
}

/// Returns the validated sample rate used by the default microphone.
pub(super) fn default_microphone_sample_rate() -> Result<u32, String> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or_else(|| "Микрофон Windows не найден".to_owned())?;
    let config = device
        .default_input_config()
        .map_err(|_| "Не удалось прочитать настройки микрофона".to_owned())?;
    validate_sample_rate(config.sample_rate().0)
}

fn record_microphone(
    recording: &AtomicBool,
    mut consume: impl FnMut(Vec<i16>) -> Result<(), String>,
) -> Result<u32, String> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or_else(|| "Микрофон Windows не найден".to_owned())?;
    let config = device
        .default_input_config()
        .map_err(|_| "Не удалось прочитать настройки микрофона".to_owned())?;
    let rate = validate_sample_rate(config.sample_rate().0)?;
    let channels = usize::from(config.channels());
    let (chunks_tx, chunks_rx) = mpsc::sync_channel(32);
    let error = |_| log::warn!("microphone capture error");
    let stream = match config.sample_format() {
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config.config(),
            move |data: &[i16], _| send_mono_i16(&chunks_tx, data, channels),
            error,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config.config(),
            move |data: &[u16], _| {
                let converted: Vec<i16> = data.iter().map(|v| (*v as i32 - 32768) as i16).collect();
                send_mono_i16(&chunks_tx, &converted, channels)
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
                send_mono_i16(&chunks_tx, &converted, channels)
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
        if let Ok(chunk) = chunks_rx.recv_timeout(Duration::from_millis(20)) {
            consume(chunk)?;
        }
    }
    drop(stream);
    while let Ok(chunk) = chunks_rx.try_recv() {
        consume(chunk)?;
    }
    Ok(rate)
}

fn validate_sample_rate(rate: u32) -> Result<u32, String> {
    if matches!(rate, 8_000 | 16_000 | 24_000 | 48_000) {
        Ok(rate)
    } else {
        Err(format!(
            "Микрофон использует неподдерживаемую частоту {rate} Hz"
        ))
    }
}

fn send_mono_i16(target: &mpsc::SyncSender<Vec<i16>>, input: &[i16], channels: usize) {
    let chunk = input
        .chunks(channels.max(1))
        .map(|frame| {
            (frame.iter().map(|sample| i32::from(*sample)).sum::<i32>() / frame.len().max(1) as i32)
                as i16
        })
        .collect();
    if target.try_send(chunk).is_err() {
        log::warn!("microphone audio chunk dropped because the voice sender is busy");
    }
}
