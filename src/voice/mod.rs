//! RockServer voice WebSocket client.

mod dto;
mod rank;
mod record;

use std::{
    collections::HashSet,
    net::ToSocketAddrs,
    sync::Arc,
    time::Duration,
};

use tungstenite::Message;

use crate::stations::Station;

use dto::{VoiceAction, VoiceEvent};
use rank::rerank_voice_candidates;
use record::record_default_microphone;

const MAX_CHUNK: usize = 32 * 1024;
const MIN_VOICE_CANDIDATE_SCORE: f64 = 0.35;

pub struct VoiceSearchResult {
    pub stations: Vec<Station>,
    pub auto_play: bool,
}

#[derive(Debug)]
pub enum VoiceError {
    ServerUnavailable,
    TokenMissing,
    TokenInvalid,
    Message(String),
}

impl std::fmt::Display for VoiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ServerUnavailable => write!(f, "RockServer is unavailable"),
            Self::TokenMissing => write!(f, "RockServer token is not configured"),
            Self::TokenInvalid => write!(f, "RockServer token is invalid"),
            Self::Message(message) => f.write_str(message),
        }
    }
}

impl From<String> for VoiceError {
    fn from(message: String) -> Self {
        let normalized = message.to_lowercase();
        if normalized.contains("401")
            || normalized.contains("403")
            || normalized.contains("unauthorized")
            || normalized.contains("invalid token")
            || normalized.contains("токен") && normalized.contains("не настроен")
        {
            if normalized.contains("не настроен") {
                Self::TokenMissing
            } else {
                Self::TokenInvalid
            }
        } else {
            Self::Message(message)
        }
    }
}

impl From<&str> for VoiceError {
    fn from(message: &str) -> Self {
        Self::from(message.to_owned())
    }
}

/// Records one short PCM16 mono command and resolves it through RockServer.
pub fn capture_and_recognize(
    base_url: &str,
    bearer_token: &str,
    locale: &str,
    recording: Arc<std::sync::atomic::AtomicBool>,
) -> Result<VoiceSearchResult, VoiceError> {
    log::info!("voice capture started: locale={locale} base_url={base_url}");
    let (audio, sample_rate) = record_default_microphone(&recording)?;
    let url = websocket_url(base_url)?;
    let bearer_token = bearer_token.trim();
    if bearer_token.is_empty() {
        return Err("Токен RockServer не настроен".into());
    }
    log::info!(
        "voice capture finished: bytes={} sample_rate_hz={} websocket={url}",
        audio.len(),
        sample_rate
    );
    let host_port = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("wss://"))
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("127.0.0.1:3000");
    let tcp = std::net::TcpStream::connect_timeout(
        &host_port
            .to_socket_addrs()
            .map_err(|_| VoiceError::ServerUnavailable)?
            .next()
            .ok_or_else(|| "RockServer voice: не удалось разрешить адрес".to_owned())?,
        Duration::from_secs(5),
    )
    .map_err(|_| VoiceError::ServerUnavailable)?;
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(5)));
    let ws_key = tungstenite::handshake::client::generate_key();
    let request = tungstenite::http::Request::builder()
        .uri(&url)
        .header("Host", host_port)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", &ws_key)
        .header("Authorization", format!("Bearer {bearer_token}"))
        .body(())
        .map_err(|_| "Некорректный URL RockServer voice".to_owned())?;
    let (mut socket, _) = tungstenite::client(request, tcp).map_err(|e| {
        log::error!("voice websocket handshake failed: {e}");
        VoiceError::from(format!("RockServer voice handshake: {e}"))
    })?;
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
                normalized_query,
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
                let mut stations = stations
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
                // RockServer candidates are already roughly ordered by similarity score,
                // but we further bias ordering towards words from the transcript.
                rerank_voice_candidates(&transcript, &mut stations);
                if stations.is_empty() {
                    return Err("RockServer не нашёл станцию для команды".into());
                }
                return Ok(VoiceSearchResult {
                    stations,
                    auto_play: normalized_query.action == VoiceAction::Play,
                });
            }
            VoiceEvent::Error { message, .. } => return Err(message.into()),
            _ => {}
        }
    }
}

fn start_message(locale: &str, sample_rate: u32) -> String {
    serde_json::json!({
        "type": "start",
        "locale": locale,
        "sample_rate_hz": sample_rate,
        "limit": 30,
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

#[cfg(test)]
mod tests {
    use super::{VoiceError, start_message};

    #[test]
    fn start_message_is_valid_json() {
        let value: serde_json::Value = serde_json::from_str(&start_message("ru-RU", 48_000))
            .expect("start message must be valid JSON");
        assert_eq!(value["type"], "start");
        assert_eq!(value["locale"], "ru-RU");
        assert_eq!(value["sample_rate_hz"], 48_000);
        assert_eq!(value["limit"], 30);
    }

    #[test]
    fn classifies_token_errors_for_voice_prompts() {
        assert!(matches!(
            VoiceError::from("Токен RockServer не настроен"),
            VoiceError::TokenMissing
        ));
        assert!(matches!(
            VoiceError::from("HTTP 401 Unauthorized".to_owned()),
            VoiceError::TokenInvalid
        ));
    }
}
