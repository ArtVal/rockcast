//! RockServer voice WebSocket client.

mod dto;
mod rank;
mod record;
mod resample;

use std::{
    collections::HashSet,
    io::{Read, Write},
    net::ToSocketAddrs,
    sync::Arc,
    time::Duration,
};

use tungstenite::{Message, client_tls, stream::MaybeTlsStream};

use crate::stations::Station;

use dto::{VoiceAction, VoiceEvent};
use rank::rerank_voice_candidates;
use record::{
    default_microphone_sample_rate, record_default_microphone, stream_default_microphone,
};
use resample::{
    MonoPcm16Resampler, VOICE_SAMPLE_RATE_HZ, pcm16_bytes_to_samples, pcm16_samples_to_bytes,
    resample_pcm16_mono,
};

const MAX_CHUNK: usize = 32 * 1024;
const MIN_VOICE_CANDIDATE_SCORE: f64 = 0.35;

pub struct VoiceSearchResult {
    pub stations: Vec<Station>,
    pub auto_play: bool,
    pub control: Option<VoiceControl>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceControl {
    Stop,
    Next,
    Previous,
    PlayLast,
}

#[derive(Debug)]
pub enum VoiceError {
    ServerUnavailable,
    TokenMissing,
    TokenInvalid,
    /// Recognition succeeded but no playable station matched the command.
    NotFound,
    /// Protocol / session / transport failure (must not be voiced as "not found").
    Message(String),
}

impl std::fmt::Display for VoiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ServerUnavailable => write!(f, "RockServer is unavailable"),
            Self::TokenMissing => write!(f, "RockServer token is not configured"),
            Self::TokenInvalid => write!(f, "RockServer token is invalid"),
            Self::NotFound => write!(f, "RockServer не нашёл станцию для команды"),
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
    bearer_token: Option<&str>,
    locale: &str,
    recognizer_mode: &str,
    recording: Arc<std::sync::atomic::AtomicBool>,
) -> Result<VoiceSearchResult, VoiceError> {
    log::info!("voice capture started: locale={locale}");
    if recognizer_mode == "streaming_v3" {
        return capture_and_recognize_streaming(base_url, bearer_token, locale, recording);
    }
    let (audio, device_rate) = record_default_microphone(&recording)?;
    let samples = pcm16_bytes_to_samples(&audio);
    let audio = pcm16_samples_to_bytes(&resample_pcm16_mono(
        &samples,
        device_rate,
        VOICE_SAMPLE_RATE_HZ,
    ));
    log::info!(
        "voice capture finished: device_hz={device_rate} send_hz={VOICE_SAMPLE_RATE_HZ} bytes={}",
        audio.len()
    );
    let mut socket = connect_voice_socket(base_url, bearer_token)?;
    socket
        .send(Message::Text(
            start_message(locale, VOICE_SAMPLE_RATE_HZ, recognizer_mode).into(),
        ))
        .map_err(|_| "Не удалось начать voice session".to_owned())?;
    wait_for_voice_ready(&mut socket)?;
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
    receive_voice_result(&mut socket)
}

fn capture_and_recognize_streaming(
    base_url: &str,
    bearer_token: Option<&str>,
    locale: &str,
    recording: Arc<std::sync::atomic::AtomicBool>,
) -> Result<VoiceSearchResult, VoiceError> {
    let device_rate = default_microphone_sample_rate()?;
    let mut socket = connect_voice_socket(base_url, bearer_token)?;
    socket
        .send(Message::Text(
            start_message(locale, VOICE_SAMPLE_RATE_HZ, "streaming_v3").into(),
        ))
        .map_err(|_| "Не удалось начать voice session".to_owned())?;
    wait_for_voice_ready(&mut socket)?;
    log::info!(
        "voice session ready before microphone capture: device_hz={device_rate} send_hz={VOICE_SAMPLE_RATE_HZ}"
    );
    let mut resampler = MonoPcm16Resampler::new(device_rate, VOICE_SAMPLE_RATE_HZ);
    let mut sent_bytes = 0usize;
    let mut sent_chunks = 0usize;
    let _recorded_rate = stream_default_microphone(&recording, |audio| {
        let samples = pcm16_bytes_to_samples(audio);
        let resampled = pcm16_samples_to_bytes(&resampler.push(&samples));
        for chunk in resampled.chunks(MAX_CHUNK) {
            if chunk.is_empty() {
                continue;
            }
            socket
                .send(Message::Binary(chunk.to_vec().into()))
                .map_err(|_| "Не удалось отправить потоковое аудио".to_owned())?;
            sent_bytes += chunk.len();
            sent_chunks += 1;
        }
        Ok(())
    })?;
    socket
        .send(Message::Text(r#"{"type":"commit"}"#.into()))
        .map_err(|_| "Не удалось завершить voice session".to_owned())?;
    log::info!(
        "streaming voice audio committed: bytes={sent_bytes} chunks={sent_chunks} sample_rate_hz={VOICE_SAMPLE_RATE_HZ}"
    );
    receive_voice_result(&mut socket)
}

fn connect_voice_socket(
    base_url: &str,
    bearer_token: Option<&str>,
) -> Result<tungstenite::WebSocket<MaybeTlsStream<std::net::TcpStream>>, VoiceError> {
    let url = websocket_url(base_url)?;
    let (host, port, host_header) = voice_socket_endpoint(&url)?;
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| VoiceError::from(format!("RockServer voice DNS {host}:{port}: {error}")))?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err("RockServer voice: не удалось разрешить адрес".into());
    }
    let mut last_error = None;
    let mut tcp = None;
    for address in addresses {
        match std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5)) {
            Ok(stream) => {
                tcp = Some(stream);
                break;
            }
            Err(error) => last_error = Some(format!("{address}: {error}")),
        }
    }
    let tcp = tcp.ok_or_else(|| {
        VoiceError::from(format!(
            "RockServer voice TCP connect {host}:{port}: {}",
            last_error.unwrap_or_else(|| "unknown error".to_owned())
        ))
    })?;
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(5)));
    let ws_key = tungstenite::handshake::client::generate_key();
    let request = voice_handshake_request(&url, &host_header, &ws_key, bearer_token)?;
    let (socket, _) = client_tls(request, tcp).map_err(|e| {
        log::error!("voice websocket handshake failed: {e}");
        VoiceError::from(format!("RockServer voice handshake: {e}"))
    })?;
    log::info!("voice websocket connected");
    Ok(socket)
}

fn voice_handshake_request(
    url: &str,
    host_header: &str,
    ws_key: &str,
    bearer_token: Option<&str>,
) -> Result<tungstenite::http::Request<()>, VoiceError> {
    let mut request = tungstenite::http::Request::builder()
        .uri(url)
        .header("Host", host_header)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", ws_key);
    if let Some(token) = bearer_token
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let request = request
        .body(())
        .map_err(|_| "Некорректный URL RockServer voice".to_owned())?;
    Ok(request)
}

fn wait_for_voice_ready<S: Read + Write>(
    socket: &mut tungstenite::WebSocket<S>,
) -> Result<(), VoiceError> {
    loop {
        let Message::Text(text) = socket
            .read()
            .map_err(|_| "RockServer не подтвердил voice session".to_owned())?
        else {
            continue;
        };
        let event: VoiceEvent = serde_json::from_str(&text)
            .map_err(|_| "RockServer вернул некорректный voice ответ".to_owned())?;
        match event {
            VoiceEvent::Ready {} => {
                log::info!("voice session ready");
                return Ok(());
            }
            VoiceEvent::Error { message, .. } => {
                log::warn!("voice session rejected before audio: {message}");
                return Err(message.into());
            }
            other => {
                log::warn!("voice session ignored unexpected event before ready: {other:?}");
            }
        }
    }
}

fn receive_voice_result<S: Read + Write>(
    socket: &mut tungstenite::WebSocket<S>,
) -> Result<VoiceSearchResult, VoiceError> {
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
                if is_final && let Some(control) = classify_voice_control(&transcript) {
                    log::info!("voice control recognized from final transcript: {control:?}");
                    return Ok(voice_control_result(control));
                }
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
                if let Some(control) = classify_voice_control(&transcript) {
                    log::info!("voice control recognized from result transcript: {control:?}");
                    return Ok(voice_control_result(control));
                }
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
                    return Err(VoiceError::NotFound);
                }
                return Ok(VoiceSearchResult {
                    stations,
                    auto_play: normalized_query.action == VoiceAction::Play,
                    control: None,
                });
            }
            VoiceEvent::Error { message, .. } => return Err(message.into()),
            _ => {}
        }
    }
}

fn voice_control_result(control: VoiceControl) -> VoiceSearchResult {
    VoiceSearchResult {
        stations: Vec::new(),
        auto_play: false,
        control: Some(control),
    }
}

fn classify_voice_control(transcript: &str) -> Option<VoiceControl> {
    let normalized = transcript.to_lowercase();
    let words: Vec<&str> = normalized
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .filter(|word| {
            !matches!(
                *word,
                "\u{043f}\u{043e}\u{0436}\u{0430}\u{043b}\u{0443}\u{0439}\u{0441}\u{0442}\u{0430}"
                    | "please"
            )
        })
        .collect();

    match words.as_slice() {
        [
            "\u{0432}\u{043a}\u{043b}\u{044e}\u{0447}\u{0438}",
            "\u{043c}\u{0443}\u{0437}\u{044b}\u{043a}\u{0443}",
        ]
        | [
            "\u{0432}\u{043a}\u{043b}\u{044e}\u{0447}\u{0438}\u{0442}\u{044c}",
            "\u{043c}\u{0443}\u{0437}\u{044b}\u{043a}\u{0443}",
        ]
        | [
            "\u{0432}\u{043a}\u{043b}\u{044e}\u{0447}\u{0438}",
            "\u{043c}\u{0443}\u{0437}\u{044b}\u{043a}\u{0430}",
        ]
        | ["play", "music"]
        | ["play", "some", "music"]
        | ["turn", "on", "music"] => Some(VoiceControl::PlayLast),
        ["\u{0441}\u{0442}\u{043e}\u{043f}"]
        | ["\u{043e}\u{0441}\u{0442}\u{0430}\u{043d}\u{043e}\u{0432}\u{0438}"]
        | ["\u{043e}\u{0441}\u{0442}\u{0430}\u{043d}\u{043e}\u{0432}\u{0438}\u{0442}\u{044c}"]
        | ["\u{0432}\u{044b}\u{043a}\u{043b}\u{044e}\u{0447}\u{0438}"]
        | ["\u{0432}\u{044b}\u{043a}\u{043b}\u{044e}\u{0447}\u{0438}\u{0442}\u{044c}"]
        | ["stop"]
        | ["pause"]
        | ["turn", "off"] => Some(VoiceControl::Stop),
        ["\u{0434}\u{0430}\u{043b}\u{044c}\u{0448}\u{0435}"]
        | ["\u{0441}\u{043b}\u{0435}\u{0434}\u{0443}\u{044e}\u{0449}\u{0430}\u{044f}"]
        | ["\u{0441}\u{043b}\u{0435}\u{0434}\u{0443}\u{044e}\u{0449}\u{0438}\u{0439}"]
        | ["\u{0432}\u{043f}\u{0435}\u{0440}\u{0435}\u{0434}"]
        | ["next"]
        | ["next", "station"]
        | ["skip"] => Some(VoiceControl::Next),
        ["\u{043d}\u{0430}\u{0437}\u{0430}\u{0434}"]
        | ["\u{043f}\u{0440}\u{0435}\u{0434}\u{044b}\u{0434}\u{0443}\u{0449}\u{0430}\u{044f}"]
        | ["\u{043f}\u{0440}\u{0435}\u{0434}\u{044b}\u{0434}\u{0443}\u{0449}\u{0438}\u{0439}"]
        | ["previous"]
        | ["back"]
        | ["previous", "station"] => Some(VoiceControl::Previous),
        _ => None,
    }
}

fn start_message(locale: &str, sample_rate: u32, recognizer_mode: &str) -> String {
    serde_json::json!({
        "type": "start",
        "locale": locale,
        "sample_rate_hz": sample_rate,
        "recognizer_mode": recognizer_mode,
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
    Ok(format!("{scheme}/v1/voice/stream"))
}

/// Extracts the TCP endpoint from a WebSocket URL, supplying the standard port
/// when the configured RockServer URL does not explicitly contain one.
fn voice_socket_endpoint(url: &str) -> Result<(String, u16, String), String> {
    let uri = url
        .parse::<tungstenite::http::Uri>()
        .map_err(|_| "Invalid RockServer voice WebSocket URL".to_owned())?;
    let authority = uri
        .authority()
        .ok_or_else(|| "RockServer voice WebSocket URL has no host".to_owned())?;
    let port = authority.port_u16().unwrap_or(match uri.scheme_str() {
        Some("wss") => 443,
        Some("ws") => 80,
        _ => return Err("RockServer voice WebSocket URL has an unsupported scheme".to_owned()),
    });
    Ok((
        authority.host().to_owned(),
        port,
        authority.as_str().to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        VOICE_SAMPLE_RATE_HZ, VoiceControl, VoiceError, classify_voice_control, start_message,
        voice_handshake_request, voice_socket_endpoint, websocket_url,
    };

    #[test]
    fn start_message_is_valid_json() {
        let value: serde_json::Value =
            serde_json::from_str(&start_message("ru-RU", VOICE_SAMPLE_RATE_HZ, "streaming_v3"))
                .expect("start message must be valid JSON");
        assert_eq!(value["type"], "start");
        assert_eq!(value["locale"], "ru-RU");
        assert_eq!(value["sample_rate_hz"], 16_000);
        assert_eq!(value["recognizer_mode"], "streaming_v3");
        assert_eq!(value["limit"], 30);
    }

    #[test]
    fn voice_websocket_endpoint_uses_default_ports() {
        let secure_url = websocket_url("https://alex.vault57.ru").unwrap();
        assert_eq!(secure_url, "wss://alex.vault57.ru/v1/voice/stream");
        assert_eq!(
            voice_socket_endpoint(&secure_url).unwrap(),
            (
                "alex.vault57.ru".to_owned(),
                443,
                "alex.vault57.ru".to_owned()
            )
        );

        let plain_url = websocket_url("http://rockserver.local").unwrap();
        assert_eq!(plain_url, "ws://rockserver.local/v1/voice/stream");
        assert_eq!(
            voice_socket_endpoint(&plain_url).unwrap(),
            (
                "rockserver.local".to_owned(),
                80,
                "rockserver.local".to_owned()
            )
        );
    }

    #[test]
    fn public_voice_handshake_has_no_authorization_header() {
        let request = voice_handshake_request(
            "wss://alex.vault57.ru/v1/voice/stream",
            "alex.vault57.ru",
            "test-websocket-key",
            None,
        )
        .unwrap();
        assert_eq!(request.uri().scheme_str(), Some("wss"));
        assert_eq!(request.uri().path(), "/v1/voice/stream");
        assert!(request.headers().get("Authorization").is_none());
    }

    #[test]
    fn developer_voice_override_can_add_bearer() {
        let request = voice_handshake_request(
            "ws://127.0.0.1:3000/v1/voice/stream",
            "127.0.0.1:3000",
            "test-websocket-key",
            Some("dev-test-token"),
        )
        .unwrap();
        assert_eq!(request.headers()["Authorization"], "Bearer dev-test-token");
    }

    #[test]
    fn voice_websocket_endpoint_preserves_explicit_port() {
        let url = websocket_url("https://alex.vault57.ru:8443").unwrap();
        assert_eq!(
            voice_socket_endpoint(&url).unwrap(),
            (
                "alex.vault57.ru".to_owned(),
                8443,
                "alex.vault57.ru:8443".to_owned()
            )
        );
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

    #[test]
    fn protocol_sample_rate_errors_stay_as_message() {
        assert!(matches!(
            VoiceError::from("sample_rate_hz must be 16000 for /v1/voice/stream".to_owned()),
            VoiceError::Message(_)
        ));
    }

    #[test]
    fn empty_station_miss_is_not_found() {
        assert_eq!(
            VoiceError::NotFound.to_string(),
            "RockServer не нашёл станцию для команды"
        );
    }

    #[test]
    fn recognizes_russian_voice_controls() {
        assert_eq!(
            classify_voice_control("\u{0441}\u{0442}\u{043e}\u{043f}"),
            Some(VoiceControl::Stop)
        );
        assert_eq!(
            classify_voice_control("\u{0434}\u{0430}\u{043b}\u{044c}\u{0448}\u{0435}"),
            Some(VoiceControl::Next)
        );
        assert_eq!(
            classify_voice_control("\u{043d}\u{0430}\u{0437}\u{0430}\u{0434}"),
            Some(VoiceControl::Previous)
        );
    }

    #[test]
    fn recognizes_english_voice_controls() {
        assert_eq!(classify_voice_control("stop"), Some(VoiceControl::Stop));
        assert_eq!(
            classify_voice_control("next station"),
            Some(VoiceControl::Next)
        );
        assert_eq!(
            classify_voice_control("previous station"),
            Some(VoiceControl::Previous)
        );
    }

    #[test]
    fn recognizes_play_last_station_commands() {
        assert_eq!(
            classify_voice_control(
                "\u{0432}\u{043a}\u{043b}\u{044e}\u{0447}\u{0438} \u{043c}\u{0443}\u{0437}\u{044b}\u{043a}\u{0443}"
            ),
            Some(VoiceControl::PlayLast)
        );
        assert_eq!(
            classify_voice_control("play music"),
            Some(VoiceControl::PlayLast)
        );
    }
}
