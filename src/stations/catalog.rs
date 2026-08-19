//! Local stations.txt catalog parsing and ordering.

use super::Station;

/// Embedded catalog (if no file next to the exe).
const EMBEDDED_STATIONS: &str = include_str!("../../stations.txt");

pub(crate) fn catalog_stations() -> Vec<Station> {
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
    // Create an editable copy in the app data directory.
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
    crate::settings::app_dir().map(|dir| dir.join("stations.txt"))
}

/// Line format: `name | url | tags | bitrate | codec | country`
pub fn parse_stations_txt(raw: &str) -> Vec<Station> {
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

pub(crate) fn order_stations(stations: Vec<Station>) -> Vec<Station> {
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

pub(crate) fn dedupe(stations: Vec<Station>) -> Vec<Station> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for s in stations {
        let key = s.url.trim_end_matches('/').to_lowercase();
        if seen.insert(key) {
            out.push(s);
        }
    }
    out
}

pub fn infer_codec(codec: &str, url: &str) -> String {
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
    if lower_url.contains("aacp")
        || lower_url.ends_with(".aac")
        || lower_url.contains(".aac?")
        || lower_url.contains("/aac")
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
