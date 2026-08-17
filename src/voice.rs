//! Windows microphone capture and bounded RockServer voice WebSocket client.
use crate::stations::Station;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Deserialize;
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tungstenite::{Message, connect};

const MAX_RECORDING: Duration = Duration::from_secs(60);
const MAX_CHUNK: usize = 32 * 1024;
const MIN_VOICE_CANDIDATE_SCORE: f64 = 0.35;

/// Records one short PCM16 mono command and resolves it through RockServer.
pub fn capture_and_recognize(
    base_url: &str,
    locale: &str,
    recording: Arc<AtomicBool>,
) -> Result<Vec<Station>, String> {
    log::info!("voice capture started: locale={locale} base_url={base_url}");
    let (audio, sample_rate) = record_default_microphone(&recording)?;
    let url = websocket_url(base_url)?;
    log::info!(
        "voice capture finished: bytes={} sample_rate_hz={} websocket={url}",
        audio.len(),
        sample_rate
    );
    let (mut socket, _) =
        connect(url).map_err(|_| "Не удалось подключиться к RockServer voice".to_owned())?;
    log::info!("voice websocket connected");
    socket
        .send(Message::Text(start_message(locale, sample_rate).into()))
        .map_err(|_| "Не удалось начать voice session".to_owned())?;
    let _ = socket
        .read()
        .map_err(|_| "RockServer не подтвердил voice session".to_owned())?;
    for chunk in audio.chunks(MAX_CHUNK) {
        socket
            .send(Message::Binary(chunk.to_vec().into()))
            .map_err(|_| "Не удалось отправить аудио".to_owned())?;
    }
    socket
        .send(Message::Text(r#"{"type":"commit"}"#.into()))
        .map_err(|_| "Не удалось завершить voice session".to_owned())?;
    log::info!(
        "voice audio sent: bytes={} chunks={}",
        audio.len(),
        audio.len().div_ceil(MAX_CHUNK)
    );
    loop {
        let Message::Text(text) = socket
            .read()
            .map_err(|_| "RockServer завершил voice session".to_owned())?
        else {
            continue;
        };
        let event: VoiceEvent = serde_json::from_str(&text)
            .map_err(|_| "RockServer вернул некорректный voice ответ".to_owned())?;
        match event {
            VoiceEvent::Transcript {
                transcript,
                is_final,
                ..
            } => {
                log::info!("voice transcript: final={is_final} text={transcript:?}");
            }
            VoiceEvent::Result {
                transcript,
                stations,
                ..
            } => {
                log::info!(
                    "voice result: transcript={transcript:?} candidates={}",
                    stations.len()
                );
                for (index, station) in stations.iter().enumerate() {
                    log::info!(
                        "voice candidate[{index}]: name={:?} country={:?} score={} url={}",
                        station.name,
                        station.country_code,
                        station.score,
                        station.stream_url
                    );
                }
                let mut seen_streams = HashSet::new();
                let stations = stations
                    .into_iter()
                    .filter(|station| {
                        let stream_key = station
                            .stream_url
                            .trim()
                            .trim_end_matches('/')
                            .to_ascii_lowercase();
                        let unique = seen_streams.insert(stream_key);
                        let accepted = unique
                            && station.score >= MIN_VOICE_CANDIDATE_SCORE
                            && !station.stream_url.contains(".example.com")
                            && !station.stream_url.contains("example.test");
                        if !accepted {
                            log::warn!(
                                "voice candidate rejected: name={:?} score={} duplicate={} url={}",
                                station.name,
                                station.score,
                                !unique,
                                station.stream_url
                            );
                        }
                        accepted
                    })
                    .map(Station::from)
                    .collect::<Vec<_>>();
                if stations.is_empty() {
                    return Err("RockServer не нашёл станцию для команды".into());
                }
                return Ok(stations);
            }
            VoiceEvent::Error { message, .. } => return Err(message),
            _ => {}
        }
    }
}

fn start_message(locale: &str, sample_rate: u32) -> String {
    serde_json::json!({
        "type": "start",
        "locale": locale,
        "sample_rate_hz": sample_rate,
        "limit": 10,
    })
    .to_string()
}

fn websocket_url(base: &str) -> Result<String, String> {
    let base = base.trim().trim_end_matches('/');
    let scheme = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        return Err("RockServer URL must start with http:// or https://".into());
    };
    Ok(format!("{scheme}/api/v1/voice/stream"))
}

fn record_default_microphone(recording: &AtomicBool) -> Result<(Vec<u8>, u32), String> {
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
    while recording.load(Ordering::Acquire) && started.elapsed() < MAX_RECORDING {
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
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum VoiceEvent {
    Ready {},
    Transcript {
        transcript: String,
        is_final: bool,
    },
    Result {
        transcript: String,
        #[serde(default)]
        stations: Vec<StationDto>,
    },
    Error {
        message: String,
    },
}
#[derive(Deserialize)]
struct StationDto {
    name: String,
    stream_url: String,
    #[serde(default)]
    tags: Vec<String>,
    bitrate_kbps: Option<u32>,
    codec: Option<String>,
    country_code: Option<String>,
    score: f64,
}
impl From<StationDto> for Station {
    fn from(v: StationDto) -> Self {
        Self {
            name: v.name,
            url: v.stream_url,
            tags: v.tags.join(", "),
            bitrate: v.bitrate_kbps.unwrap_or(0),
            codec: v.codec.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_message_is_valid_json() {
        let value: serde_json::Value = serde_json::from_str(&start_message("ru-RU", 48_000))
            .expect("start message must be valid JSON");
        assert_eq!(value["type"], "start");
        assert_eq!(value["locale"], "ru-RU");
        assert_eq!(value["sample_rate_hz"], 48_000);
        assert_eq!(value["limit"], 10);
    }
}
