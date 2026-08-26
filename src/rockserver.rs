//! Bounded RockServer HTTP search client; microphone streaming is layered separately.
use crate::stations::Station;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Public RockServer used by official RockCast releases.
pub(crate) const PRODUCTION_BASE_URL: &str = "https://alex.vault57.ru";

/// Runtime-only server configuration. It is deliberately not serializable or
/// printable so an override credential cannot leak into user settings or logs.
#[derive(Clone)]
pub(crate) struct RuntimeConfig {
    base_url: String,
    bearer_token: Option<String>,
    recognizer_mode: &'static str,
}

impl RuntimeConfig {
    pub(crate) fn for_app() -> Self {
        let production = Self {
            base_url: PRODUCTION_BASE_URL.to_owned(),
            bearer_token: None,
            recognizer_mode: "buffered_v1",
        };
        #[cfg(debug_assertions)]
        {
            let Ok(base_url) = std::env::var("ROCKCAST_DEV_ROCKSERVER_URL") else {
                return production;
            };
            let base_url = base_url.trim();
            if base_url.is_empty() {
                return production;
            }
            let bearer_token = std::env::var("ROCKCAST_DEV_ROCKSERVER_BEARER_TOKEN")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty());
            let recognizer_mode = match std::env::var("ROCKCAST_DEV_ROCKSERVER_VOICE_MODE") {
                Ok(value) if value.trim() == "streaming_v3" => "streaming_v3",
                _ => "buffered_v1",
            };
            Self {
                base_url: base_url.to_owned(),
                bearer_token,
                recognizer_mode,
            }
        }
        #[cfg(not(debug_assertions))]
        production
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(crate) fn bearer_token(&self) -> Option<&str> {
        self.bearer_token.as_deref()
    }

    pub(crate) fn recognizer_mode(&self) -> &'static str {
        self.recognizer_mode
    }

    #[cfg(test)]
    fn for_test(base_url: String, bearer_token: Option<&str>) -> Self {
        Self {
            base_url,
            bearer_token: bearer_token.map(str::to_owned),
            recognizer_mode: "buffered_v1",
        }
    }
}

/// Queries RockServer and converts its public station DTOs into playback stations.
pub(crate) fn search(
    config: &RuntimeConfig,
    query: &str,
    locale: &str,
) -> Result<Vec<Station>, String> {
    let base = config.base_url().trim().trim_end_matches('/');
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err("RockServer URL must start with http:// or https://".into());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| "RockServer HTTP client failed".to_owned())?;
    let mut request = client
        .post(format!("{base}/v1/search"))
        .json(&SearchRequest {
            query,
            locale,
            limit: 50,
        });
    if let Some(token) = config.bearer_token() {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .map_err(|_| "RockServer is unavailable; using local catalog".to_owned())?;
    if !response.status().is_success() {
        return Err(format!(
            "RockServer returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let body: SearchResponse = response
        .json()
        .map_err(|_| "RockServer returned invalid search JSON".to_owned())?;
    Ok(body
        .stations
        .into_iter()
        .map(|item| {
            let url = item.stream_url;
            let mut station = Station::from_primary(
                item.id,
                item.name,
                url,
                item.tags.join(", "),
                item.country_code.unwrap_or_default(),
                item.bitrate_kbps.unwrap_or(0),
                item.codec.unwrap_or_default(),
            );
            station.homepage_url = item.homepage_url;
            station.favicon_url = item.favicon_url;
            station
        })
        .collect())
}

#[derive(Serialize)]
struct SearchRequest<'a> {
    query: &'a str,
    locale: &'a str,
    limit: u8,
}
#[derive(Deserialize)]
struct SearchResponse {
    stations: Vec<StationDto>,
}
#[derive(Deserialize)]
struct StationDto {
    id: String,
    name: String,
    stream_url: String,
    #[serde(default)]
    tags: Vec<String>,
    bitrate_kbps: Option<u32>,
    codec: Option<String>,
    country_code: Option<String>,
    #[serde(default, alias = "homepageUrl")]
    homepage_url: Option<String>,
    #[serde(default, alias = "faviconUrl")]
    favicon_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn serve_one(response_body: &'static str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8(request).unwrap()
        });
        (format!("http://{address}"), handle)
    }

    #[test]
    fn public_search_uses_v1_without_authorization() {
        let (base_url, server) = serve_one(r#"{"stations":[]}"#);
        let config = RuntimeConfig::for_test(base_url, None);
        assert!(search(&config, "rock", "en").unwrap().is_empty());
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("post /v1/search http/1.1\r\n"));
        assert!(!request.contains("authorization:"));
    }

    #[test]
    fn developer_override_can_add_bearer_without_changing_route() {
        let (base_url, server) = serve_one(r#"{"stations":[]}"#);
        let config = RuntimeConfig::for_test(base_url, Some("dev-test-token"));
        assert!(search(&config, "", "ru").unwrap().is_empty());
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("post /v1/search http/1.1\r\n"));
        assert!(request.contains("authorization: bearer dev-test-token\r\n"));
    }

    #[test]
    fn production_endpoint_is_https() {
        assert_eq!(PRODUCTION_BASE_URL, "https://alex.vault57.ru");
    }
}
