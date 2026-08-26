//! Microphone capture for voice commands.

use std::{
    sync::{Arc, Mutex, atomic::AtomicBool, mpsc},
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
    let sample_rate = validate_sample_rate(config.sample_rate().0)?;
    let channels = usize::from(config.channels());
    let samples = Arc::new(Mutex::new(Vec::<i16>::new()));
    let output = Arc::clone(&samples);
    let error = |_| log::warn!("microphone capture error");
    let stream = match config.sample_format() {
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config.config(),
            move |data: &[i16], _| push_mono_i16(&output, data, channels),
            error,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config.config(),
            move |data: &[u16], _| {
                let converted: Vec<i16> = data
                    .iter()
                    .map(|value| (*value as i32 - 32768) as i16)
                    .collect();
                push_mono_i16(&output, &converted, channels)
            },
            error,
            None,
        ),
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.config(),
            move |data: &[f32], _| {
                let converted: Vec<i16> = data
                    .iter()
                    .map(|value| (value.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                    .collect();
                push_mono_i16(&output, &converted, channels)
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
    // Device rate is resampled to 16 kHz before sending to RockServer.
    // Accept common capture rates (including 44.1 kHz on Linux / PulseAudio).
    if (8_000..=192_000).contains(&rate) {
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

fn push_mono_i16(target: &Mutex<Vec<i16>>, input: &[i16], channels: usize) {
    if let Ok(mut target) = target.lock() {
        target.extend(input.chunks(channels.max(1)).map(|frame| {
            (frame.iter().map(|sample| i32::from(*sample)).sum::<i32>() / frame.len().max(1) as i32)
                as i16
        }));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::{push_mono_i16, validate_sample_rate};

    #[test]
    fn buffered_capture_keeps_every_mixed_frame() {
        let samples = Mutex::new(Vec::new());

        push_mono_i16(&samples, &[100, 300, -100, 100, 200, 400], 2);

        assert_eq!(*samples.lock().unwrap(), [200, 0, 300]);
    }

    #[test]
    fn accepts_cd_quality_and_rejects_nonsense_rates() {
        assert_eq!(validate_sample_rate(44_100).unwrap(), 44_100);
        assert_eq!(validate_sample_rate(48_000).unwrap(), 48_000);
        assert!(validate_sample_rate(7_999).is_err());
        assert!(validate_sample_rate(192_001).is_err());
    }
}
