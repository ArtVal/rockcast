//! Bounded RockServer HTTP search client; microphone streaming is layered separately.
use crate::stations::Station;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Public RockServer used by official RockCast releases.
pub(crate) const PRODUCTION_BASE_URL: &str = "https://rockplatform.win";

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

    pub(crate) fn station_stream_url(&self, station_id: &str) -> Result<String, String> {
        if station_id.is_empty()
            || station_id.len() > 128
            || matches!(station_id, "." | "..")
            || station_id.chars().any(char::is_control)
        {
            return Err("Invalid station ID".into());
        }
        let mut url = reqwest::Url::parse(&format!(
            "{}/api/v1/stations/",
            self.base_url.trim().trim_end_matches('/')
        ))
        .map_err(|_| "Invalid RockServer URL")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("Invalid RockServer URL".into());
        }
        url.path_segments_mut()
            .map_err(|_| "Invalid RockServer URL")?
            .pop_if_empty()
            .push(station_id)
            .push("stream");
        Ok(url.into())
    }

    pub(crate) fn is_station_stream_url(&self, url: &str) -> bool {
        let Ok(prefix) = self.station_stream_url("station") else {
            return false;
        };
        let prefix = prefix.strip_suffix("station/stream").unwrap();
        let Some(id) = url
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix("/stream"))
        else {
            return false;
        };
        !id.is_empty() && !id.contains(['/', '?', '#'])
    }

    pub(crate) fn bearer_token(&self) -> Option<&str> {
        self.bearer_token.as_deref()
    }

    pub(crate) fn recognizer_mode(&self) -> &'static str {
        self.recognizer_mode
    }

    #[cfg(test)]
    pub(crate) fn for_test(base_url: String, bearer_token: Option<&str>) -> Self {
        Self {
            base_url,
            bearer_token: bearer_token.map(str::to_owned),
            recognizer_mode: "buffered_v1",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SearchBatch {
    pub stations: Vec<Station>,
    pub total: usize,
    pub has_more: bool,
}

/// Queries RockServer and converts its public station DTOs into playback stations.
pub(crate) fn search(
    config: &RuntimeConfig,
    query: &str,
    locale: &str,
    limit: usize,
    offset: usize,
) -> Result<SearchBatch, String> {
    let base = config.base_url().trim().trim_end_matches('/');
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err("RockServer URL must start with http:// or https://".into());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| "RockServer HTTP client failed".to_owned())?;
    let mut request = client
        .post(format!("{base}/api/v1/search"))
        .json(&SearchRequest {
            query,
            locale,
            limit,
            offset,
        });
    if let Some(token) = config.bearer_token() {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .map_err(|e| format!("RockServer is unavailable: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "RockServer returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let body: SearchResponse = response
        .json()
        .map_err(|_| "RockServer returned invalid search JSON".to_owned())?;
    let stations: Vec<Station> = body
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
            station.language = item.language;
            station
        })
        .collect();
    let n = stations.len();
    let total = body.total.unwrap_or(offset + n);
    let has_more = body.has_more.unwrap_or(false);
    Ok(SearchBatch {
        stations,
        total,
        has_more,
    })
}

/// Queries RockServer for a single station by its stable ID (`GET /api/v1/catalog/stations/{station_id}`).
pub(crate) fn get_station(config: &RuntimeConfig, station_id: &str) -> Result<Station, String> {
    let station_id = station_id.trim();
    if station_id.is_empty() {
        return Err("Station ID cannot be empty".into());
    }
    let base = config.base_url().trim().trim_end_matches('/');
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err("RockServer URL must start with http:// or https://".into());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| "RockServer HTTP client failed".to_owned())?;
    let mut request = client.get(format!("{base}/api/v1/catalog/stations/{station_id}"));
    if let Some(token) = config.bearer_token() {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .map_err(|e| format!("RockServer is unavailable: {e}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("Station '{station_id}' was not found on server"));
    }
    if !response.status().is_success() {
        return Err(format!(
            "RockServer returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let item: StationDto = response
        .json()
        .map_err(|_| "RockServer returned invalid station JSON".to_owned())?;
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
    station.language = item.language;
    Ok(station)
}

#[derive(Debug, Clone)]
pub(crate) struct CatalogBatch {
    pub stations: Vec<Station>,
    pub next_cursor: Option<String>,
}

/// Lists paginated stations from the RockServer catalog (`GET /api/v1/catalog/stations`).
pub(crate) fn list_catalog(
    config: &RuntimeConfig,
    cursor: Option<&str>,
    limit: usize,
) -> Result<CatalogBatch, String> {
    let base = config.base_url().trim().trim_end_matches('/');
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err("RockServer URL must start with http:// or https://".into());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| "RockServer HTTP client failed".to_owned())?;
    let mut url = format!("{base}/api/v1/catalog/stations?limit={limit}");
    if let Some(c) = cursor {
        let trimmed = c.trim();
        if !trimmed.is_empty() {
            let encoded: String = trimmed
                .bytes()
                .flat_map(|b| match b {
                    b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                        vec![b as char]
                    }
                    _ => format!("%{:02X}", b).chars().collect(),
                })
                .collect();
            url.push_str(&format!("&cursor={encoded}"));
        }
    }
    let mut request = client.get(url);
    if let Some(token) = config.bearer_token() {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .map_err(|e| format!("RockServer is unavailable: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "RockServer returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let body: CatalogResponse = response
        .json()
        .map_err(|_| "RockServer returned invalid catalog JSON".to_owned())?;
    let stations: Vec<Station> = body
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
            station.language = item.language;
            station
        })
        .collect();
    Ok(CatalogBatch {
        stations,
        next_cursor: body.next_cursor,
    })
}

const MAX_CATALOG_PAGES: usize = 2;
const CATALOG_PAGE_LIMIT: usize = 50;

/// Loads the starter catalog from RockServer online (matching rockmobile/rock-esp32).
/// Fetches up to 2 pages of 50 stations (100 total) and orders them with rock priority.
pub(crate) fn load_catalogue(
    config: &RuntimeConfig,
) -> Result<(Vec<Station>, Option<String>), String> {
    let mut stations = Vec::new();
    let mut seen_cursors = std::collections::HashSet::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let batch = list_catalog(config, cursor.as_deref(), CATALOG_PAGE_LIMIT)?;
        stations.extend(batch.stations);
        cursor = batch.next_cursor;
        pages += 1;
        if cursor
            .as_ref()
            .is_some_and(|c| !seen_cursors.insert(c.clone()))
        {
            return Err("RockServer catalogue cursor repeated".into());
        }
        if cursor.is_none() || pages >= MAX_CATALOG_PAGES {
            break;
        }
    }
    let mut seen_ids = std::collections::HashSet::new();
    stations.retain(|s| seen_ids.insert(s.id.clone()));
    let ordered = crate::stations::order_stations(stations);
    Ok((ordered, cursor))
}

#[derive(Serialize)]
struct SearchRequest<'a> {
    query: &'a str,
    locale: &'a str,
    limit: usize,
    offset: usize,
}
#[derive(Deserialize)]
struct SearchResponse {
    stations: Vec<StationDto>,
    #[serde(default)]
    total: Option<usize>,
    #[serde(default)]
    has_more: Option<bool>,
}
#[derive(Deserialize)]
struct CatalogResponse {
    stations: Vec<StationDto>,
    #[serde(default)]
    next_cursor: Option<String>,
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
    #[serde(default)]
    language: Option<String>,
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

    #[test]
    fn relay_url_encodes_station_id_and_limits_credentials_to_its_endpoint() {
        let config = RuntimeConfig::for_test("https://server.test/".into(), None);
        let url = config.station_stream_url("radio/рок ?#").unwrap();
        assert_eq!(
            url,
            "https://server.test/api/v1/stations/radio%2F%D1%80%D0%BE%D0%BA%20%3F%23/stream"
        );
        assert!(config.is_station_stream_url(&url));
        for url in [
            "https://server.test.evil/api/v1/stations/id/stream",
            "http://server.test/api/v1/stations/id/stream",
            "https://server.test/api/v1/stations/id/stream?token=x",
            "https://server.test/api/v1/stations/a/b/stream",
            "https://server.test/api/v1/stations//stream",
            "https://upstream.test/live.mp3",
        ] {
            assert!(!config.is_station_stream_url(url), "{url}");
        }
        for id in ["", ".", "..", "a\nb", &"я".repeat(65)] {
            assert!(config.station_stream_url(id).is_err());
        }
    }

    fn serve_one(response_body: &'static str) -> (String, thread::JoinHandle<String>) {
        serve_status("200 OK", response_body)
    }

    fn serve_status(
        status_line: &'static str,
        response_body: &'static str,
    ) -> (String, thread::JoinHandle<String>) {
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
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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
        let (base_url, server) = serve_one(r#"{"stations":[],"total":0,"has_more":false}"#);
        let config = RuntimeConfig::for_test(base_url, None);
        assert!(
            search(&config, "rock", "en", 20, 0)
                .unwrap()
                .stations
                .is_empty()
        );
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("post /api/v1/search http/1.1\r\n"));
        assert!(!request.contains("authorization:"));
    }

    #[test]
    fn search_parses_total_and_has_more_batch() {
        let body = r#"{"stations":[{"id":"st-1","name":"Station 1","stream_url":"http://stream/1"}],"total":42,"has_more":true}"#;
        let (base_url, server) = serve_one(body);
        let config = RuntimeConfig::for_test(base_url, None);
        let batch = search(&config, "rock", "en", 20, 0).unwrap();
        assert_eq!(batch.stations.len(), 1);
        assert_eq!(batch.stations[0].id, "st-1");
        assert_eq!(batch.total, 42);
        assert!(batch.has_more);
        let _ = server.join();
    }

    #[test]
    fn public_get_station_uses_catalog_endpoint_without_authorization() {
        let body = r#"{"id":"rock-fm","name":"Rock FM","stream_url":"https://stream.test/rock","tags":["rock"],"country_code":"US","bitrate_kbps":320,"codec":"mp3"}"#;
        let (base_url, server) = serve_one(body);
        let config = RuntimeConfig::for_test(base_url, None);
        let station = get_station(&config, "rock-fm").unwrap();
        assert_eq!(station.id, "rock-fm");
        assert_eq!(station.name, "Rock FM");
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /api/v1/catalog/stations/rock-fm http/1.1\r\n"));
        assert!(!request.contains("authorization:"));
    }

    #[test]
    fn get_station_returns_error_on_404() {
        let (base_url, server) = serve_status("404 Not Found", r#"{"error":"not_found"}"#);
        let config = RuntimeConfig::for_test(base_url, None);
        let err = get_station(&config, "unknown").unwrap_err();
        assert!(err.contains("not found"));
        let _ = server.join();
    }

    #[test]
    fn developer_override_can_add_bearer_without_changing_route() {
        let (base_url, server) = serve_one(r#"{"stations":[]}"#);
        let config = RuntimeConfig::for_test(base_url, Some("dev-test-token"));
        assert!(
            search(&config, "", "ru", 20, 0)
                .unwrap()
                .stations
                .is_empty()
        );
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("post /api/v1/search http/1.1\r\n"));
        assert!(request.contains("authorization: bearer dev-test-token\r\n"));
    }

    #[test]
    fn public_list_catalog_uses_catalog_endpoint_without_authorization() {
        let (base_url, server) = serve_one(r#"{"stations":[],"next_cursor":null}"#);
        let config = RuntimeConfig::for_test(base_url, None);
        assert!(list_catalog(&config, None, 20).unwrap().stations.is_empty());
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /api/v1/catalog/stations?limit=20 http/1.1\r\n"));
        assert!(!request.contains("authorization:"));
    }

    #[test]
    fn list_catalog_parses_cursor_and_batch() {
        let body = r#"{"stations":[{"id":"st-1","name":"Station 1","stream_url":"http://stream/1"}],"next_cursor":"cursor-next"}"#;
        let (base_url, server) = serve_one(body);
        let config = RuntimeConfig::for_test(base_url, None);
        let batch = list_catalog(&config, Some("cursor-prev"), 10).unwrap();
        assert_eq!(batch.stations.len(), 1);
        assert_eq!(batch.stations[0].id, "st-1");
        assert_eq!(batch.next_cursor.as_deref(), Some("cursor-next"));
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(
            request.starts_with(
                "get /api/v1/catalog/stations?limit=10&cursor=cursor-prev http/1.1\r\n"
            )
        );
    }

    #[test]
    fn load_catalogue_fetches_and_orders_stations() {
        let body = r#"{"stations":[{"id":"st-pop","name":"Pop Hits","stream_url":"https://stream/pop","tags":["pop"]},{"id":"st-metal","name":"Metal Rock","stream_url":"https://stream/metal","tags":["metal"]}],"next_cursor":null}"#;
        let (base_url, server) = serve_one(body);
        let config = RuntimeConfig::for_test(base_url, None);
        let (stations, next_cursor) = load_catalogue(&config).unwrap();
        assert_eq!(stations.len(), 2);
        assert_eq!(stations[0].id, "st-metal");
        assert_eq!(stations[1].id, "st-pop");
        assert!(next_cursor.is_none());
        let _ = server.join();
    }

    #[test]
    fn production_endpoint_is_https() {
        assert_eq!(PRODUCTION_BASE_URL, "https://rockplatform.win");
    }
}
