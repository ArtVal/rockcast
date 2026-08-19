//! Rock / metal radio stations: local catalog + Radio Browser API.

use std::{
    collections::HashSet,
    io::{Read, Write},
    net::TcpStream,
    sync::Arc,
    time::Duration,
};

use rustls::pki_types::ServerName;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Station {
    pub name: String,
    pub url: String,
    pub tags: String,
    pub bitrate: u32,
    pub codec: String,
}

impl Station {
    pub fn content_type(&self) -> &'static str {
        let c = self.effective_codec();
        if matches!(c.as_str(), "aac" | "aac+" | "he-aac" | "aacp") {
            "audio/aac"
        } else if c == "opus" {
            "audio/ogg; codecs=opus"
        } else if c == "vorbis" {
            "audio/ogg; codecs=vorbis"
        } else if c == "ogg" {
            "audio/ogg"
        } else if c == "flac" {
            "audio/flac"
        } else {
            "audio/mpeg"
        }
    }

    fn effective_codec(&self) -> String {
        infer_codec(&self.codec, &self.url)
    }
}

use crate::i18n::{self, Lang};

/// Fallback entry points if DNS/servers are unavailable.
const FALLBACK_HOSTS: &[&str] = &["all.api.radio-browser.info"];

/// Embedded catalog (if no file next to the exe).
const EMBEDDED_STATIONS: &str = include_str!("../stations.txt");

fn catalog_stations() -> Vec<Station> {
    let raw = read_stations_file().unwrap_or_else(|| EMBEDDED_STATIONS.to_string());
    parse_stations_txt(&raw)
}

fn read_stations_file() -> Option<String> {
    for path in stations_search_paths() {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            log::info!("stations.txt: {}", path.display());
            return Some(raw);
        }
    }
    // Create an editable copy in AppData.
    if let Some(path) = appdata_stations_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&path, EMBEDDED_STATIONS).is_ok() {
            log::info!("stations.txt created: {}", path.display());
            return Some(EMBEDDED_STATIONS.to_string());
        }
    }
    None
}

fn stations_search_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Ok(p) = std::env::var("ROCKCAST_STATIONS") {
        paths.push(std::path::PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        paths.push(dir.join("stations.txt"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join("stations.txt"));
    }
    if let Some(p) = appdata_stations_path() {
        paths.push(p);
    }
    paths
}

fn appdata_stations_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(
        std::path::PathBuf::from(base)
            .join("RockCast")
            .join("stations.txt"),
    )
}

/// Line format: `name | url | tags | bitrate | codec | country`
fn parse_stations_txt(raw: &str) -> Vec<Station> {
    let mut out = Vec::new();
    for (lineno, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('|').map(str::trim).collect();
        if parts.len() < 2 {
            log::warn!("stations.txt:{lineno}: need at least name|url");
            continue;
        }
        let name = parts[0].to_string();
        let url = parts[1].to_string();
        if name.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
            log::warn!("stations.txt:{lineno}: skip (empty name or URL)");
            continue;
        }
        let tags = parts.get(2).copied().unwrap_or("").to_string();
        let bitrate = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(128);
        let codec = parts.get(4).copied().unwrap_or("mp3").to_string();
        out.push(Station {
            name,
            url,
            tags,
            bitrate,
            codec,
        });
    }
    out
}

/// Instant local list (no network).
pub fn load_catalog(lang: Lang) -> (Vec<Station>, String) {
    let ordered = order_stations(catalog_stations());
    let n = ordered.len();
    (ordered, i18n::fmt1(lang.t().local_catalog, n))
}

/// Enrich the catalog via Radio Browser (may take several seconds).
pub fn enrich_stations(catalog: Vec<Station>, lang: Lang) -> (Vec<Station>, String) {
    let hosts = discover_hosts();
    log::info!("Radio Browser hosts: {}", hosts.join(", "));

    for host in &hosts {
        match fetch_tag(host, "metal", 40) {
            Ok(chunks) if !chunks.is_empty() => {
                let mut merged = chunks;
                merged.extend(catalog);
                let merged = order_stations(dedupe(merged));
                let n = merged.len();
                let limited: Vec<_> = merged.into_iter().take(120).collect();
                return (limited, i18n::fmt1(lang.t().catalog_plus_rb, n));
            }
            Ok(_) => log::info!("Radio Browser {host}: empty response"),
            Err(e) => log::info!("Radio Browser {host}: {e}"),
        }
    }
    let n = catalog.len();
    (
        order_stations(catalog),
        i18n::fmt1(lang.t().local_catalog, n),
    )
}

#[derive(Debug, Deserialize)]
struct RbServer {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RbStation {
    name: Option<String>,
    url: Option<String>,
    url_resolved: Option<String>,
    tags: Option<String>,
    #[serde(default)]
    bitrate: Option<serde_json::Value>,
    codec: Option<String>,
}

fn resolve_ipv4(host: &str, port: u16) -> Option<std::net::SocketAddr> {
    use std::net::ToSocketAddrs;
    (host, port).to_socket_addrs().ok()?.find(|a| a.is_ipv4())
}

/// HTTPS GET over IPv4 only (no hyper/reqwest — IPv6 is broken here and corrupts the response body).
fn https_get_ipv4(host: &str, path_and_query: &str) -> Result<Vec<u8>, String> {
    let addr = resolve_ipv4(host, 443).ok_or_else(|| format!("no IPv4 for {host}"))?;
    log::debug!("Radio Browser TCP {host} → {addr}");

    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(8))
        .map_err(|e| format!("tcp {addr}: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_nodelay(true);

    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let server_name = ServerName::try_from(host.to_string()).map_err(|e| format!("sni: {e}"))?;
    let conn = rustls::ClientConnection::new(Arc::new(config), server_name)
        .map_err(|e| format!("tls: {e}"))?;
    let mut tls = rustls::StreamOwned::new(conn, stream);

    // HTTP/1.0 — no chunked encoding; body until connection close.
    let request = format!(
        "GET {path_and_query} HTTP/1.0\r\n\
         Host: {host}\r\n\
         User-Agent: RockCast/0.1\r\n\
         Accept: application/json\r\n\
         Accept-Encoding: identity\r\n\
         Connection: close\r\n\
         \r\n"
    );
    tls.write_all(request.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    tls.flush().map_err(|e| format!("flush: {e}"))?;

    let mut raw = Vec::new();
    tls.read_to_end(&mut raw)
        .map_err(|e| format!("read: {e}"))?;
    if raw.is_empty() {
        return Err("empty response".into());
    }

    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| {
            let head = String::from_utf8_lossy(&raw[..raw.len().min(120)]);
            format!("missing HTTP headers: {head:?}")
        })?;
    let header_bytes = &raw[..sep];
    let body = raw[sep + 4..].to_vec();
    let headers = String::from_utf8_lossy(header_bytes);
    let status_line = headers.lines().next().unwrap_or("");
    if !status_line.contains(" 200") {
        return Err(format!("HTTP {status_line}"));
    }
    Ok(body)
}

fn discover_hosts() -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let push = |name: &str, out: &mut Vec<String>, seen: &mut HashSet<String>| {
        let name = name
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_lowercase();
        let host = name.split(['/', ':', '?']).next().unwrap_or("").trim();
        if host.is_empty() {
            return;
        }
        if seen.insert(host.to_string()) {
            out.push(host.to_string());
        }
    };

    push("all.api.radio-browser.info", &mut out, &mut seen);

    match https_get_ipv4("all.api.radio-browser.info", "/json/servers") {
        Ok(body) => match serde_json::from_slice::<Vec<RbServer>>(&body) {
            Ok(list) => {
                for s in list {
                    if let Some(name) = s.name {
                        push(&name, &mut out, &mut seen);
                    }
                }
            }
            Err(e) => log::info!("Radio Browser servers json: {e}"),
        },
        Err(e) => log::info!("Radio Browser servers: {e}"),
    }

    if out.is_empty() {
        for h in FALLBACK_HOSTS {
            push(h, &mut out, &mut seen);
        }
    }
    out
}

fn is_playlist(url: &str) -> bool {
    let path = url.split('?').next().unwrap_or(url).to_lowercase();
    path.ends_with(".m3u")
        || path.ends_with(".m3u8")
        || path.ends_with(".pls")
        || path.ends_with(".asx")
        || path.ends_with(".xspf")
}

fn bitrate_u32(v: Option<serde_json::Value>) -> u32 {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0) as u32,
        Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn normalize(raw: RbStation) -> Option<Station> {
    let name = raw.name?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let url = [raw.url_resolved, raw.url]
        .into_iter()
        .flatten()
        .map(|u| u.trim().to_string())
        .find(|u| (u.starts_with("http://") || u.starts_with("https://")) && !is_playlist(u))?;
    Some(Station {
        name,
        url,
        tags: raw.tags.unwrap_or_default(),
        bitrate: bitrate_u32(raw.bitrate),
        codec: raw.codec.unwrap_or_default(),
    })
}

fn fetch_tag(host: &str, tag: &str, limit: u32) -> Result<Vec<Station>, String> {
    let path = format!(
        "/json/stations/search?tag={tag}&hidebroken=true&order=clickcount&reverse=true&limit={limit}"
    );
    let mut last_err = None;
    for attempt in 1..=3 {
        match fetch_tag_once(host, &path) {
            Ok(stations) if !stations.is_empty() => return Ok(stations),
            Ok(_) => last_err = Some("empty station list".into()),
            Err(e) => {
                log::info!("Radio Browser {host} attempt {attempt}: {e}");
                last_err = Some(e);
            }
        }
        std::thread::sleep(Duration::from_millis(250 * attempt as u64));
    }
    Err(last_err.unwrap_or_else(|| "failed to load".into()))
}

fn fetch_tag_once(host: &str, path: &str) -> Result<Vec<Station>, String> {
    let bytes = https_get_ipv4(host, path)?;
    if bytes.is_empty() {
        return Err("empty response".into());
    }
    let values: Vec<serde_json::Value> = serde_json::from_slice(&bytes).map_err(|e| {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(80)]);
        format!("json ({} bytes, head={head:?}): {e}", bytes.len())
    })?;
    Ok(values
        .into_iter()
        .filter_map(|v| serde_json::from_value::<RbStation>(v).ok())
        .filter_map(normalize)
        .collect())
}

fn dedupe(stations: Vec<Station>) -> Vec<Station> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for s in stations {
        let key = s.url.trim_end_matches('/').to_lowercase();
        if seen.insert(key) {
            out.push(s);
        }
    }
    out
}

fn order_stations(stations: Vec<Station>) -> Vec<Station> {
    let metal_tags = [
        "metal",
        "hard rock",
        "punk",
        "thrash",
        "death",
        "doom",
        "industrial",
    ];
    let mut metal = Vec::new();
    let mut others = Vec::new();
    for s in stations {
        let tags = s.tags.to_lowercase();
        if metal_tags.iter().any(|t| tags.contains(t)) {
            metal.push(s);
        } else {
            others.push(s);
        }
    }
    let key = |s: &Station| (!s.url.starts_with("https://"), s.name.to_lowercase());
    metal.sort_by_key(|s| key(s));
    others.sort_by_key(|s| key(s));
    metal.extend(others);
    metal
}

fn infer_codec(codec: &str, url: &str) -> String {
    let normalized = codec.trim().to_lowercase();
    let lower_url = url.to_lowercase();
    if lower_url.ends_with(".opus") || lower_url.contains(".opus?") {
        return "opus".into();
    }
    if lower_url.ends_with(".ogg") || lower_url.contains(".ogg?") {
        return if normalized.is_empty() {
            "ogg".into()
        } else {
            normalized
        };
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::Station;

    fn station(codec: &str) -> Station {
        Station {
            name: "test".into(),
            url: "https://example.test/stream".into(),
            tags: String::new(),
            bitrate: 128,
            codec: codec.into(),
        }
    }

    #[test]
    fn opus_station_uses_codec_specific_ogg_type() {
        assert_eq!(station("opus").content_type(), "audio/ogg; codecs=opus");
    }

    #[test]
    fn vorbis_station_uses_codec_specific_ogg_type() {
        assert_eq!(station("vorbis").content_type(), "audio/ogg; codecs=vorbis");
    }

    #[test]
    fn opus_url_overrides_generic_ogg_codec() {
        let mut s = station("ogg");
        s.url = "http://play.global.audio/avtoradio.opus".into();
        assert_eq!(s.content_type(), "audio/ogg; codecs=opus");
    }

    #[test]
    fn opus_url_overrides_missing_codec() {
        let mut s = station("");
        s.url = "http://play.global.audio/avtoradio.opus".into();
        assert_eq!(s.content_type(), "audio/ogg; codecs=opus");
    }
}
