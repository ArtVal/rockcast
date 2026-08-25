//! Versioned station catalog loader with a temporary legacy TXT adapter.

use super::{Station, StationStream};
use serde::Deserialize;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

pub const PINNED_CATALOG_VERSION: &str = "2026.08.2";
pub const PINNED_CATALOG_SHA256: &str =
    "3fa20dca94fc059bd433a47b9fba9bb6d5e5e1aa2957a5ffb58b2a7b20b1d74d";
/// Remove in RM-004-I after the first schema-v1 release has completed one release
/// cycle, and never before 2026-10-31.
pub const LEGACY_TXT_REMOVAL: &str =
    "RM-004-I after one schema-v1 release cycle, not before 2026-10-31";
const EMBEDDED_CATALOG: &str = include_str!("../../assets/catalog/stations.v1.json");
const EMBEDDED_MANIFEST: &str = include_str!("../../assets/catalog/manifest.json");

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CatalogError {
    #[error("invalid JSON: {0}")]
    Json(String),
    #[error("unsupported schema version {0}")]
    SchemaVersion(u32),
    #[error("catalog checksum mismatch")]
    Checksum,
    #[error("invalid catalog: {0}")]
    Invalid(String),
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    schema_version: u32,
    catalog_version: String,
    stations: Vec<CatalogStation>,
    #[serde(default)]
    tombstones: Vec<CatalogTombstone>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogTombstone {
    id: String,
    reason: String,
    replacement_ids: Vec<String>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogStation {
    id: String,
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    legacy_ids: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    country_code: Option<String>,
    language: Option<String>,
    homepage_url: Option<String>,
    favicon_url: Option<String>,
    streams: Vec<CatalogStream>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogStream {
    id: String,
    url: String,
    codec: Option<String>,
    bitrate_kbps: Option<u32>,
    primary: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    algorithm: String,
    catalog_version: String,
    file: String,
    sha256: String,
}

pub(crate) fn catalog_stations() -> Vec<Station> {
    catalog_snapshot().0
}

/// Resolver input comes only from the accepted local catalog snapshot.  TXT has
/// no lifecycle data, so it deliberately supplies active IDs only.
pub(crate) fn catalog_resolver() -> crate::personal_data::CatalogResolver {
    catalog_snapshot().1
}

fn catalog_snapshot() -> (Vec<Station>, crate::personal_data::CatalogResolver) {
    for path in stations_search_paths() {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        match parse_override_with_resolver(&path, &raw) {
            Ok((stations, resolver)) => {
                log::info!("station catalog override: {}", path.display());
                return (stations, resolver);
            }
            Err(error) => log::warn!(
                "station catalog override {} rejected: {error}",
                path.display()
            ),
        }
    }
    if let Some(path) = appdata_catalog_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&path, EMBEDDED_CATALOG).is_ok() {
            log::info!("station catalog created: {}", path.display());
        }
    }
    let stations = parse_embedded_catalog().expect("vendored RM-004 catalog must be valid");
    let document = parse_document(EMBEDDED_CATALOG).expect("vendored RM-004 catalog must be valid");
    let resolver = resolver_from_document(&stations, document);
    (stations, resolver)
}

fn resolver_from_document(
    stations: &[Station],
    document: Document,
) -> crate::personal_data::CatalogResolver {
    let tombstones = document
        .tombstones
        .into_iter()
        .filter_map(|t| {
            let reason = match t.reason.as_str() {
                "removed" => crate::personal_data::TombstoneReason::Removed,
                "merged" => crate::personal_data::TombstoneReason::Merged,
                "split" => crate::personal_data::TombstoneReason::Split,
                _ => return None,
            };
            Some(crate::personal_data::Tombstone {
                id: t.id,
                reason,
                replacement_ids: t.replacement_ids,
            })
        })
        .collect();
    crate::personal_data::CatalogResolver::from_stations(stations, Some(document.catalog_version))
        .with_tombstones(tombstones)
}

fn parse_override_with_resolver(
    path: &Path,
    raw: &str,
) -> Result<(Vec<Station>, crate::personal_data::CatalogResolver), CatalogError> {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
        || !raw.trim_start().starts_with('{')
    {
        let stations = parse_stations_txt(raw);
        let resolver = crate::personal_data::CatalogResolver::from_stations(&stations, None);
        Ok((stations, resolver))
    } else {
        let document = parse_document(raw)?;
        // Keep the public schema parser as the single station-validation path;
        // the second parsed document carries only lifecycle metadata for profiles.
        let stations = parse_stations_json(raw)?;
        let resolver = resolver_from_document(&stations, document);
        Ok((stations, resolver))
    }
}

fn parse_embedded_catalog() -> Result<Vec<Station>, CatalogError> {
    let manifest: Manifest =
        serde_json::from_str(EMBEDDED_MANIFEST).map_err(|e| CatalogError::Json(e.to_string()))?;
    if manifest.algorithm != "sha256"
        || manifest.file != "stations.v1.json"
        || manifest.catalog_version != PINNED_CATALOG_VERSION
        || manifest.sha256 != PINNED_CATALOG_SHA256
        || sha256_hex(&canonical_catalog_bytes(EMBEDDED_CATALOG)) != PINNED_CATALOG_SHA256
    {
        return Err(CatalogError::Checksum);
    }
    let doc = parse_document(EMBEDDED_CATALOG)?;
    if doc.catalog_version != PINNED_CATALOG_VERSION {
        return Err(CatalogError::Invalid(
            "pinned catalogVersion mismatch".into(),
        ));
    }
    stations_from_document(doc)
}

/// Parses a forward-compatible schema-v1 JSON override; unknown fields are ignored.
pub fn parse_stations_json(raw: &str) -> Result<Vec<Station>, CatalogError> {
    stations_from_document(parse_document(raw)?)
}
fn parse_document(raw: &str) -> Result<Document, CatalogError> {
    let doc: Document = serde_json::from_str(raw).map_err(|e| CatalogError::Json(e.to_string()))?;
    if doc.schema_version != 1 {
        return Err(CatalogError::SchemaVersion(doc.schema_version));
    }
    if doc.catalog_version.trim().is_empty() {
        return Err(CatalogError::Invalid("catalogVersion is empty".into()));
    }
    Ok(doc)
}
fn stations_from_document(doc: Document) -> Result<Vec<Station>, CatalogError> {
    if doc.stations.is_empty() {
        return Err(CatalogError::Invalid("stations is empty".into()));
    }
    let mut ids = HashSet::new();
    doc.stations
        .into_iter()
        .map(|station| {
            if !is_stable_id(&station.id) || !ids.insert(station.id.clone()) {
                return Err(CatalogError::Invalid(format!(
                    "invalid or duplicate station id {}",
                    station.id
                )));
            }
            if station.name.trim().is_empty() || station.name != station.name.trim() {
                return Err(CatalogError::Invalid(format!(
                    "invalid station name for {}",
                    station.id
                )));
            }
            if station
                .streams
                .iter()
                .filter(|stream| stream.primary)
                .count()
                != 1
            {
                return Err(CatalogError::Invalid(format!(
                    "{} must have exactly one primary stream",
                    station.id
                )));
            }
            let mut stream_ids = HashSet::new();
            let mut streams = Vec::with_capacity(station.streams.len());
            for stream in station.streams {
                if !is_stable_id(&stream.id)
                    || !stream_ids.insert(stream.id.clone())
                    || !is_http_url(&stream.url)
                {
                    return Err(CatalogError::Invalid(format!(
                        "invalid stream in {}",
                        station.id
                    )));
                }
                streams.push(StationStream {
                    id: stream.id,
                    url: stream.url,
                    codec: stream.codec.unwrap_or_default(),
                    bitrate: stream.bitrate_kbps.unwrap_or(0),
                    primary: stream.primary,
                });
            }
            let primary = streams
                .iter()
                .find(|stream| stream.primary)
                .expect("checked")
                .clone();
            Ok(Station {
                id: station.id,
                name: station.name,
                url: primary.url,
                tags: station.tags.join(", "),
                country: station.country_code.unwrap_or_default(),
                language: station.language,
                homepage_url: station.homepage_url,
                favicon_url: station.favicon_url,
                aliases: station.aliases,
                legacy_ids: station.legacy_ids,
                bitrate: primary.bitrate,
                codec: primary.codec,
                streams,
            })
        })
        .collect()
}
fn is_stable_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 96
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
        && !id.contains("--")
}
fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}
fn stations_search_paths() -> Vec<PathBuf> {
    let environment = std::env::var("ROCKCAST_STATIONS").ok().map(PathBuf::from);
    let executable = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let working_directory = std::env::current_dir().ok();
    override_paths(
        environment,
        executable,
        working_directory,
        crate::settings::app_dir(),
    )
}

fn override_paths(
    environment: Option<PathBuf>,
    executable: Option<PathBuf>,
    working_directory: Option<PathBuf>,
    app_data: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut paths = environment.into_iter().collect::<Vec<_>>();
    for directory in [executable, working_directory, app_data]
        .into_iter()
        .flatten()
    {
        paths.extend(source_paths(&directory));
    }
    paths
}
fn source_paths(dir: &Path) -> [PathBuf; 2] {
    [dir.join("stations.v1.json"), dir.join("stations.txt")]
}
fn appdata_catalog_path() -> Option<PathBuf> {
    crate::settings::app_dir().map(|dir| dir.join("stations.v1.json"))
}

/// Existing TXT overrides remain supported only during the transition described by
/// LEGACY_TXT_REMOVAL. Format: name | url | tags | bitrate | codec | country.
pub fn parse_stations_txt(raw: &str) -> Vec<Station> {
    log::warn!(
        "stations.txt transition adapter in use; remove {}",
        LEGACY_TXT_REMOVAL
    );
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
        let (name, url) = (parts[0].to_owned(), parts[1].to_owned());
        if name.is_empty() || !is_http_url(&url) {
            log::warn!("stations.txt:{lineno}: skip (empty name or URL)");
            continue;
        }
        out.push(Station::from_primary(
            format!("legacy-{}", &sha256_hex(url.as_bytes())[..16]),
            name,
            url,
            parts.get(2).copied().unwrap_or("").to_owned(),
            parts.get(5).copied().unwrap_or("").to_owned(),
            parts
                .get(3)
                .and_then(|value| value.parse().ok())
                .unwrap_or(128),
            parts.get(4).copied().unwrap_or("mp3").to_owned(),
        ));
    }
    out
}

/// The catalog release tool hashes canonical UTF-8/LF JSON. Git may materialize
/// text assets with CRLF on Windows, so verification uses that same canonical form.
fn canonical_catalog_bytes(raw: &str) -> Vec<u8> {
    let mut canonical = raw.replace("\r\n", "\n");
    canonical.truncate(canonical.trim_end_matches('\n').len());
    canonical.push('\n');
    canonical.into_bytes()
}
pub(crate) fn order_stations(stations: Vec<Station>) -> Vec<Station> {
    let tags = [
        "metal",
        "hard rock",
        "punk",
        "thrash",
        "death",
        "doom",
        "industrial",
    ];
    let (mut metal, mut others): (Vec<_>, Vec<_>) = stations.into_iter().partition(|station| {
        tags.iter()
            .any(|tag| station.tags.to_lowercase().contains(tag))
    });
    let key = |station: &Station| {
        (
            !station.url.starts_with("https://"),
            station.name.to_lowercase(),
        )
    };
    metal.sort_by_key(key);
    others.sort_by_key(key);
    metal.extend(others);
    metal
}
pub(crate) fn dedupe(stations: Vec<Station>) -> Vec<Station> {
    let mut seen = HashSet::new();
    stations
        .into_iter()
        .filter(|station| seen.insert(station.url.trim_end_matches('/').to_lowercase()))
        .collect()
}
pub fn infer_codec(codec: &str, url: &str) -> String {
    let normalized = codec.trim().to_lowercase();
    let url = url.to_lowercase();
    if url.ends_with(".opus") || url.contains(".opus?") {
        return "opus".into();
    }
    if url.ends_with(".ogg") || url.contains(".ogg?") {
        return if normalized.is_empty() {
            "ogg".into()
        } else {
            normalized
        };
    }
    if url.contains("aacp")
        || url.ends_with(".aac")
        || url.contains(".aac?")
        || url.contains("/aac")
    {
        return "aac+".into();
    }
    if matches!(
        normalized.as_str(),
        "aac" | "aac+" | "he-aac" | "aacp" | "he-aac-v2"
    ) {
        return normalized;
    }
    normalized
}
fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut input = bytes.to_vec();
    let length = (input.len() as u64) * 8;
    input.push(0x80);
    while !(input.len() + 8).is_multiple_of(64) {
        input.push(0)
    }
    input.extend_from_slice(&length.to_be_bytes());
    let mut state = [
        0x6a09e667_u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    for chunk in input.chunks_exact(64) {
        let mut w = [0_u32; 64];
        for (i, g) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(g.try_into().expect("four bytes"));
        }
        for i in 16..64 {
            let a = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let b = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(a)
                .wrapping_add(w[i - 7])
                .wrapping_add(b);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) = (
            state[0], state[1], state[2], state[3], state[4], state[5], state[6], state[7],
        );
        for i in 0..64 {
            let t1 = h
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                .wrapping_add((a & b) ^ (a & c) ^ (b & c));
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_catalog_is_pinned_and_offline() {
        let stations = parse_embedded_catalog().unwrap();
        assert_eq!(stations.len(), 41);
        assert_eq!(stations[0].id, "somafm-metal-detector");
    }
    #[test]
    fn json_uses_primary_without_discarding_other_streams() {
        let raw = r#"{"schemaVersion":1,"catalogVersion":"2026.08.9","stations":[{"id":"stable-id","name":"Station","tags":[],"countryCode":null,"language":null,"homepageUrl":null,"faviconUrl":null,"streams":[{"id":"backup","url":"http://example.test/backup","codec":"aac","bitrateKbps":64,"primary":false},{"id":"main","url":"https://example.test/main","codec":"mp3","bitrateKbps":128,"primary":true}]}]}"#;
        let stations = parse_stations_json(raw).unwrap();
        assert_eq!(stations[0].id, "stable-id");
        assert_eq!(stations[0].url, "https://example.test/main");
        assert_eq!(stations[0].streams.len(), 2);
    }
    #[test]
    fn schema_and_primary_failures_are_rejected() {
        assert!(matches!(
            parse_stations_json(r#"{"schemaVersion":2,"catalogVersion":"x","stations":[]}"#),
            Err(CatalogError::SchemaVersion(2))
        ));
        assert!(parse_stations_json(r#"{"schemaVersion":1,"catalogVersion":"x","stations":[{"id":"a","name":"A","streams":[{"id":"x","url":"https://x.test","primary":false}]}]}"#).is_err());
    }
    #[test]
    fn checksum_failure_is_detected() {
        assert_ne!(sha256_hex(b"changed"), PINNED_CATALOG_SHA256);
    }

    #[test]
    fn canonical_checksum_is_independent_of_windows_line_endings() {
        assert_eq!(
            sha256_hex(&canonical_catalog_bytes(EMBEDDED_CATALOG)),
            PINNED_CATALOG_SHA256
        );
    }
    #[test]
    fn txt_transition_preserves_playback_url() {
        let station =
            &parse_stations_txt("Name | https://example.test/live | rock | 128 | mp3 | US")[0];
        assert_eq!(station.url, "https://example.test/live");
        assert!(station.id.starts_with("legacy-"));
    }

    #[test]
    fn dedupe_and_order_keep_the_selected_playback_url() {
        let stations = vec![
            Station::from_primary(
                "a".into(),
                "Z".into(),
                "https://x.test/".into(),
                "rock".into(),
                "".into(),
                128,
                "mp3".into(),
            ),
            Station::from_primary(
                "b".into(),
                "A".into(),
                "https://x.test".into(),
                "metal".into(),
                "".into(),
                128,
                "mp3".into(),
            ),
        ];
        let result = order_stations(dedupe(stations));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].url, "https://x.test/");
    }

    #[test]
    fn json_then_txt_is_the_override_order_within_each_source() {
        let paths = source_paths(Path::new("override"));
        assert_eq!(paths[0], PathBuf::from("override/stations.v1.json"));
        assert_eq!(paths[1], PathBuf::from("override/stations.txt"));
    }

    #[test]
    fn environment_executable_cwd_and_appdata_keep_their_precedence() {
        let paths = override_paths(
            Some(PathBuf::from("env.json")),
            Some(PathBuf::from("exe")),
            Some(PathBuf::from("cwd")),
            Some(PathBuf::from("appdata")),
        );
        assert_eq!(
            paths,
            vec![
                PathBuf::from("env.json"),
                PathBuf::from("exe/stations.v1.json"),
                PathBuf::from("exe/stations.txt"),
                PathBuf::from("cwd/stations.v1.json"),
                PathBuf::from("cwd/stations.txt"),
                PathBuf::from("appdata/stations.v1.json"),
                PathBuf::from("appdata/stations.txt"),
            ]
        );
    }
}
