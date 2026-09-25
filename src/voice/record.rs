//! Microphone capture for voice commands.

use std::{
    sync::{Arc, Mutex, atomic::AtomicBool, mpsc},
    time::Duration,
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const MAX_RECORDING: Duration = Duration::from_secs(45);
const DEFAULT_SPEECH_THRESHOLD_RMS: u32 = 340;
const DEFAULT_SILENCE_THRESHOLD_RMS: u32 = 180;
const SILENCE_DURATION: Duration = Duration::from_millis(1000);
const MAX_SILENCE_BEFORE_SPEECH: Duration = Duration::from_millis(4500);
const STARTUP_GRACE_DURATION: Duration = Duration::from_millis(350);
const MAX_SPEECH_DURATION: Duration = Duration::from_secs(8);

/// Tracks voice activity and signals when speech has naturally concluded.
#[derive(Debug, Clone)]
pub(super) struct SpeechEndDetector {
    /// Minimum RMS required to register speech activity.
    pub(super) speech_threshold: u32,
    /// RMS below which audio is treated as silence during pause evaluation.
    pub(super) silence_threshold: u32,
    /// Duration of sustained silence required after speech before auto-stopping.
    pub(super) silence_duration: Duration,
    /// Grace period at start (ignores mouse clicks and initial prompt beep).
    pub(super) startup_grace: Duration,
    /// Maximum time to wait for initial speech before timing out.
    pub(super) max_pre_speech: Duration,
    /// Maximum duration of continuous speech before forcing a commit.
    pub(super) max_speech_duration: Duration,
    speech_started: bool,
    started_at: std::time::Instant,
    first_speech_at: Option<std::time::Instant>,
    last_speech_at: std::time::Instant,
}

impl SpeechEndDetector {
    pub(super) fn new(
        speech_threshold: u32,
        silence_threshold: u32,
        silence_duration: Duration,
        max_pre_speech: Duration,
    ) -> Self {
        let now = std::time::Instant::now();
        Self {
            speech_threshold,
            silence_threshold,
            silence_duration,
            startup_grace: STARTUP_GRACE_DURATION,
            max_pre_speech,
            max_speech_duration: MAX_SPEECH_DURATION,
            speech_started: false,
            started_at: now,
            first_speech_at: None,
            last_speech_at: now,
        }
    }

    #[cfg(test)]
    pub(super) fn is_speech_started(&self) -> bool {
        self.speech_started
    }

    /// Evaluates current audio window RMS and returns true if recording should stop.
    pub(super) fn should_stop(&mut self, rms: u32, now: std::time::Instant) -> bool {
        // Ignore during startup grace (prevents beep audio or click from triggering speech)
        if now.saturating_duration_since(self.started_at) < self.startup_grace {
            return false;
        }

        if rms >= self.speech_threshold {
            if !self.speech_started {
                log::info!(
                    "voice detector: speech started (rms={rms} >= threshold={})",
                    self.speech_threshold
                );
                self.speech_started = true;
                self.first_speech_at = Some(now);
            }
            self.last_speech_at = now;
        }

        if self.speech_started {
            // Silence is evaluated from the last moment speech was actually heard.
            // Small fluctuations between silence_threshold and speech_threshold do not reset last_speech_at.
            let silence_elapsed = now.saturating_duration_since(self.last_speech_at);
            if silence_elapsed >= self.silence_duration {
                log::info!(
                    "voice detector: silence detected after speech ({:?} >= {:?}); auto-stopping",
                    silence_elapsed,
                    self.silence_duration
                );
                return true;
            }

            if let Some(first_at) = self.first_speech_at
                && now.saturating_duration_since(first_at) >= self.max_speech_duration
            {
                log::info!(
                    "voice detector: max speech duration reached ({:?}); auto-stopping",
                    self.max_speech_duration
                );
                return true;
            }
        } else if now.saturating_duration_since(self.started_at) >= self.max_pre_speech {
            log::info!(
                "voice detector: pre-speech silence timeout ({:?}); auto-stopping",
                self.max_pre_speech
            );
            return true;
        }

        false
    }
}

/// Calculates root-mean-square amplitude of mono PCM16 samples.
fn rms_amplitude(samples: &[i16]) -> u32 {
    if samples.is_empty() {
        return 0;
    }
    let sum: u64 = samples
        .iter()
        .map(|&s| {
            let v = i64::from(s);
            (v * v) as u64
        })
        .sum();
    ((sum / samples.len() as u64) as f64).sqrt() as u32
}

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
    let window_samples = (sample_rate as usize / 10).max(1600);
    let mut calibrated = false;
    let mut min_ambient_rms = u32::MAX;
    let mut detector = SpeechEndDetector::new(
        DEFAULT_SPEECH_THRESHOLD_RMS,
        DEFAULT_SILENCE_THRESHOLD_RMS,
        SILENCE_DURATION,
        MAX_SILENCE_BEFORE_SPEECH,
    );
    while recording.load(std::sync::atomic::Ordering::Acquire) && started.elapsed() < MAX_RECORDING
    {
        std::thread::sleep(Duration::from_millis(20));
        let now = std::time::Instant::now();
        if let Ok(samples_guard) = samples.lock() {
            let len = samples_guard.len();
            if len >= window_samples {
                let window = &samples_guard[len - window_samples..];
                let rms = rms_amplitude(window);
                let elapsed = started.elapsed();
                if elapsed < STARTUP_GRACE_DURATION {
                    min_ambient_rms = min_ambient_rms.min(rms);
                } else if !calibrated {
                    let base = if min_ambient_rms == u32::MAX {
                        rms
                    } else {
                        min_ambient_rms
                    };
                    let dynamic_speech = (base + 140).clamp(280, 520);
                    let dynamic_silence = (base + 50).min(dynamic_speech.saturating_sub(50));
                    detector.speech_threshold = dynamic_speech;
                    detector.silence_threshold = dynamic_silence;
                    calibrated = true;
                    log::info!(
                        "voice detector calibrated: ambient={base} speech_thresh={dynamic_speech} silence_thresh={dynamic_silence}"
                    );
                }
                if detector.should_stop(rms, now) {
                    recording.store(false, std::sync::atomic::Ordering::Release);
                    break;
                }
            }
        }
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
    let window_samples = (rate as usize / 10).max(1600);
    let mut window_buffer: Vec<i16> = Vec::with_capacity(window_samples * 2);
    let mut calibrated = false;
    let mut min_ambient_rms = u32::MAX;
    let mut detector = SpeechEndDetector::new(
        DEFAULT_SPEECH_THRESHOLD_RMS,
        DEFAULT_SILENCE_THRESHOLD_RMS,
        SILENCE_DURATION,
        MAX_SILENCE_BEFORE_SPEECH,
    );
    while recording.load(std::sync::atomic::Ordering::Acquire) && started.elapsed() < MAX_RECORDING
    {
        if let Ok(chunk) = chunks_rx.recv_timeout(Duration::from_millis(20)) {
            window_buffer.extend_from_slice(&chunk);
            if window_buffer.len() > window_samples {
                let excess = window_buffer.len() - window_samples;
                window_buffer.drain(0..excess);
            }
            let now = std::time::Instant::now();
            let rms = rms_amplitude(&window_buffer);
            let elapsed = started.elapsed();
            if elapsed < STARTUP_GRACE_DURATION {
                min_ambient_rms = min_ambient_rms.min(rms);
            } else if !calibrated {
                let base = if min_ambient_rms == u32::MAX {
                    rms
                } else {
                    min_ambient_rms
                };
                let dynamic_speech = (base + 140).clamp(280, 520);
                let dynamic_silence = (base + 50).min(dynamic_speech.saturating_sub(50));
                detector.speech_threshold = dynamic_speech;
                detector.silence_threshold = dynamic_silence;
                calibrated = true;
                log::info!(
                    "voice streaming calibrated: ambient={base} speech_thresh={dynamic_speech} silence_thresh={dynamic_silence}"
                );
            }
            if detector.should_stop(rms, now) {
                recording.store(false, std::sync::atomic::Ordering::Release);
                consume(chunk)?;
                break;
            }
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
    use std::{sync::Mutex, time::Duration};

    use super::{SpeechEndDetector, push_mono_i16, validate_sample_rate};

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

    #[test]
    fn rms_amplitude_measures_signal_energy() {
        assert_eq!(super::rms_amplitude(&[]), 0);
        assert_eq!(super::rms_amplitude(&[0, 0, 0, 0]), 0);
        let rms = super::rms_amplitude(&[1000, -1000, 1000, -1000]);
        assert_eq!(rms, 1000);
    }

    #[test]
    fn speech_end_detector_stops_after_silence() {
        let mut detector = SpeechEndDetector::new(
            300,
            180,
            Duration::from_millis(500),
            Duration::from_millis(3000),
        );
        detector.startup_grace = Duration::ZERO;
        let t0 = std::time::Instant::now();

        // Noise below threshold does not start speech
        assert!(!detector.should_stop(100, t0));
        assert!(!detector.is_speech_started());

        // Speech occurs
        let t1 = t0 + Duration::from_millis(200);
        assert!(!detector.should_stop(450, t1));
        assert!(detector.is_speech_started());

        // Short silence does not stop
        let t2 = t1 + Duration::from_millis(300);
        assert!(!detector.should_stop(80, t2));

        // Noise in hysteresis band (between 180 and 300) does not reset speech timer
        let t3 = t1 + Duration::from_millis(400);
        assert!(!detector.should_stop(220, t3));

        // Silence exceeding threshold stops
        let t4 = t1 + Duration::from_millis(550);
        assert!(detector.should_stop(80, t4));
    }

    #[test]
    fn speech_end_detector_times_out_on_pre_speech_silence() {
        let mut detector = SpeechEndDetector::new(
            300,
            180,
            Duration::from_millis(500),
            Duration::from_millis(1500),
        );
        detector.startup_grace = Duration::ZERO;
        let t0 = std::time::Instant::now();

        assert!(!detector.should_stop(50, t0 + Duration::from_millis(500)));
        assert!(detector.should_stop(50, t0 + Duration::from_millis(1600)));
    }

    #[test]
    fn speech_end_detector_ignores_startup_grace() {
        let mut detector = SpeechEndDetector::new(
            300,
            180,
            Duration::from_millis(500),
            Duration::from_millis(3000),
        );
        detector.startup_grace = Duration::from_millis(250);
        let t0 = std::time::Instant::now();

        // Loud noise (e.g. beep prompt) during grace period does not trigger speech
        assert!(!detector.should_stop(1500, t0 + Duration::from_millis(100)));
        assert!(!detector.is_speech_started());
    }

    #[test]
    fn speech_end_detector_stops_on_max_speech_duration() {
        let mut detector = SpeechEndDetector::new(
            300,
            180,
            Duration::from_millis(500),
            Duration::from_millis(10000),
        );
        detector.startup_grace = Duration::ZERO;
        detector.max_speech_duration = Duration::from_millis(2000);
        let t0 = std::time::Instant::now();

        assert!(!detector.should_stop(500, t0 + Duration::from_millis(100)));
        assert!(detector.is_speech_started());

        // Speech continues past max speech duration
        assert!(detector.should_stop(500, t0 + Duration::from_millis(2200)));
    }
}
