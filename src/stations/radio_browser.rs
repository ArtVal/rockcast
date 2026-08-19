//! Radio Browser API enrichment.

use std::{
    collections::HashSet,
    io::{Read, Write},
    net::TcpStream,
    sync::Arc,
    time::Duration,
};

use rustls::pki_types::ServerName;
use serde::Deserialize;

use crate::i18n::{self, Lang};

use super::{catalog, Station};

/// Fallback entry points if DNS/servers are unavailable.
const FALLBACK_HOSTS: &[&str] = &["all.api.radio-browser.info"];

/// Enrich the catalog via Radio Browser (may take several seconds).
pub fn enrich_stations(catalog_stations: Vec<Station>, lang: Lang) -> (Vec<Station>, String) {
    let hosts = discover_hosts();
    log::info!("Radio Browser hosts: {}", hosts.join(", "));

    for host in &hosts {
        match fetch_tag(host, "metal", 40) {
            Ok(chunks) if !chunks.is_empty() => {
                let mut merged = chunks;
                merged.extend(catalog_stations);
                let merged = catalog::order_stations(catalog::dedupe(merged));
                let n = merged.len();
                let limited: Vec<_> = merged.into_iter().take(120).collect();
                return (limited, i18n::fmt1(lang.t().catalog_plus_rb, n));
            }
            Ok(_) => log::info!("Radio Browser {host}: empty response"),
            Err(e) => log::info!("Radio Browser {host}: {e}"),
        }
    }
    let n = catalog_stations.len();
    (
        catalog::order_stations(catalog_stations),
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
