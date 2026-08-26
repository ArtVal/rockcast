//! Direct station icon loading for the pre-RockServer-icon MVP.
//!
//! The UI only receives decoded, bounded pixels. Network I/O, image decoding,
//! and cache I/O all run on the existing [`BackgroundRuntime`]. The cache is a
//! small versioned file keyed by the station id (or stream URL for legacy
//! stations); the source URL stored in the file invalidates it when catalog
//! metadata changes.

use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use image::{ImageReader, Limits};
use reqwest::Url;

use crate::{settings, stations::Station};

const CACHE_MAGIC: &[u8] = b"RCSTICON1";
const MAX_DOWNLOAD_BYTES: usize = 512 * 1024;
const MAX_CACHE_BYTES: usize = 2 * 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 16 * 1024;
const MAX_ICON_SIDE: u32 = 64;
const MAX_DECODE_SIDE: u32 = 1024;
const MAX_DECODE_BYTES: u64 = 32 * 1024 * 1024;

/// Decoded icon payload safe to send through the app's UI channel.
#[derive(Debug, Clone)]
pub struct StationIconImage {
    pub cache_key: String,
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// App-data cache root. A relative fallback keeps headless/test environments
/// usable when no platform app-data directory is configured.
pub fn cache_dir() -> PathBuf {
    settings::app_dir()
        .map(|dir| dir.join("station-icons"))
        .unwrap_or_else(|| PathBuf::from("rockcast_station_icons"))
}

/// Return the only URL that the MVP is allowed to fetch for a station.
///
/// An explicit favicon/logo URL wins. When it is absent, the fallback is the
/// conventional `/favicon.ico` on the explicitly supplied official homepage;
/// no homepage HTML is fetched or scraped.
pub fn source_url(station: &Station) -> Option<String> {
    if let Some(explicit) = station.favicon_url.as_deref() {
        return valid_http_url(explicit).map(|url| url.to_string());
    }

    let homepage = station.homepage_url.as_deref().and_then(valid_http_url)?;
    homepage.join("/favicon.ico").ok().map(|mut url| {
        url.set_query(None);
        url.set_fragment(None);
        url.to_string()
    })
}

/// Stable in-memory identity for one station/source pair.
pub fn request_key(station: &Station, source: &str) -> String {
    format!("{}\0{}", station.id, source)
}

/// Load a valid cached icon or fetch/decode/cache it once.
pub fn load_or_fetch(station: &Station, root: &Path) -> Result<Option<StationIconImage>, String> {
    let Some(source) = source_url(station) else {
        return Ok(None);
    };
    let cache_key = cache_stem(station);
    let path = root.join(format!("{cache_key}.bin"));
    if let Some(image) = read_cache(&path, &source, &cache_key)? {
        return Ok(Some(image));
    }

    let bytes = fetch_bytes(&source)?;
    let (width, height, rgba) = decode_icon(&bytes)?;
    write_cache(&path, &source, width, height, &rgba)?;
    Ok(Some(StationIconImage {
        cache_key,
        width,
        height,
        rgba,
    }))
}

fn valid_http_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    Some(url)
}

fn fetch_bytes(source: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("RockCast station icon/1")
        .build()
        .map_err(|_| "icon HTTP client failed".to_owned())?;
    let mut response = client
        .get(source)
        .header(reqwest::header::ACCEPT, "image/*")
        .send()
        .map_err(|_| "icon download failed".to_owned())?;
    if !response.status().is_success() {
        return Err(format!(
            "icon server returned HTTP {}",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_DOWNLOAD_BYTES as u64)
    {
        return Err("icon response is too large".into());
    }

    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let count = response
            .read(&mut chunk)
            .map_err(|_| "icon response read failed".to_owned())?;
        if count == 0 {
            break;
        }
        if bytes.len().saturating_add(count) > MAX_DOWNLOAD_BYTES {
            return Err("icon response is too large".into());
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(bytes)
}

fn decode_icon(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "icon format is unsupported".to_owned())?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| "icon image is invalid".to_owned())?;
    let image = decoded.thumbnail(MAX_ICON_SIDE, MAX_ICON_SIDE).to_rgba8();
    let width = usize::try_from(image.width()).map_err(|_| "icon dimensions overflow")?;
    let height = usize::try_from(image.height()).map_err(|_| "icon dimensions overflow")?;
    let rgba = image.into_raw();
    if width == 0 || height == 0 || rgba.len() != width.saturating_mul(height).saturating_mul(4) {
        return Err("icon image has invalid dimensions".into());
    }
    Ok((width, height, rgba))
}

fn cache_stem(station: &Station) -> String {
    let material = if station.id.is_empty() {
        station.url.as_bytes()
    } else {
        station.id.as_bytes()
    };
    // Hex encoding makes the path safe even for legacy/server-provided ids.
    // The source URL is metadata inside the file and therefore invalidates the
    // cached payload when the station's icon source changes.
    let mut stem = String::with_capacity(material.len() * 2 + 4);
    stem.push_str("v1-");
    for byte in material {
        use std::fmt::Write as _;
        let _ = write!(stem, "{byte:02x}");
    }
    stem
}

fn read_cache(
    path: &Path,
    source: &str,
    cache_key: &str,
) -> Result<Option<StationIconImage>, String> {
    let Ok(bytes) = fs::read(path) else {
        return Ok(None);
    };
    if bytes.len() > MAX_CACHE_BYTES || bytes.len() < CACHE_MAGIC.len() + 16 {
        return Ok(None);
    }
    let mut cursor = 0;
    if take(&bytes, &mut cursor, CACHE_MAGIC.len()).is_none_or(|v| v != CACHE_MAGIC) {
        return Ok(None);
    }
    let width = read_u32(&bytes, &mut cursor).map_or(0, |v| v as usize);
    let height = read_u32(&bytes, &mut cursor).map_or(0, |v| v as usize);
    let source_len = read_u32(&bytes, &mut cursor).map_or(usize::MAX, |v| v as usize);
    let rgba_len = read_u32(&bytes, &mut cursor).map_or(usize::MAX, |v| v as usize);
    if width == 0
        || height == 0
        || width > MAX_ICON_SIDE as usize
        || height > MAX_ICON_SIDE as usize
        || source_len > MAX_SOURCE_BYTES
        || rgba_len != width.saturating_mul(height).saturating_mul(4)
    {
        return Ok(None);
    }
    let Some(cached_source) = take(&bytes, &mut cursor, source_len) else {
        return Ok(None);
    };
    if cached_source != source.as_bytes() {
        return Ok(None);
    }
    let Some(rgba) = take(&bytes, &mut cursor, rgba_len) else {
        return Ok(None);
    };
    if cursor != bytes.len() {
        return Ok(None);
    }
    Ok(Some(StationIconImage {
        cache_key: cache_key.to_owned(),
        width,
        height,
        rgba: rgba.to_vec(),
    }))
}

fn write_cache(
    path: &Path,
    source: &str,
    width: usize,
    height: usize,
    rgba: &[u8],
) -> Result<(), String> {
    if source.len() > MAX_SOURCE_BYTES
        || width == 0
        || height == 0
        || rgba.len() != width.saturating_mul(height).saturating_mul(4)
    {
        return Err("icon cache payload is invalid".into());
    }
    let width = u32::try_from(width).map_err(|_| "icon width overflow")?;
    let height = u32::try_from(height).map_err(|_| "icon height overflow")?;
    let source_len = u32::try_from(source.len()).map_err(|_| "icon source URL is too long")?;
    let rgba_len = u32::try_from(rgba.len()).map_err(|_| "icon payload is too large")?;
    let mut bytes = Vec::with_capacity(CACHE_MAGIC.len() + 16 + source.len() + rgba.len());
    bytes.extend_from_slice(CACHE_MAGIC);
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    bytes.extend_from_slice(&source_len.to_le_bytes());
    bytes.extend_from_slice(&rgba_len.to_le_bytes());
    bytes.extend_from_slice(source.as_bytes());
    bytes.extend_from_slice(rgba);

    fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
        .map_err(|_| "icon cache directory unavailable".to_owned())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let temporary = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
    let mut file = fs::File::create(&temporary)
        .map_err(|_| "icon cache temporary file unavailable".to_owned())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "icon cache write failed".to_owned())?;
    fs::rename(&temporary, path).map_err(|_| "icon cache commit failed".to_owned())
}

fn take<'a>(bytes: &'a [u8], cursor: &mut usize, len: usize) -> Option<&'a [u8]> {
    let end = cursor.checked_add(len)?;
    let value = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(value)
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Option<u32> {
    Some(u32::from_le_bytes(take(bytes, cursor, 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station() -> Station {
        Station::from_primary(
            "station/with\\unsafe".into(),
            "Test".into(),
            "https://stream.example.test/live".into(),
            String::new(),
            String::new(),
            128,
            "mp3".into(),
        )
    }

    #[test]
    fn explicit_favicon_wins_over_homepage() {
        let mut station = station();
        station.homepage_url = Some("https://radio.example.test/home".into());
        station.favicon_url = Some("https://cdn.example.test/logo.png?size=64".into());
        assert_eq!(
            source_url(&station).as_deref(),
            Some("https://cdn.example.test/logo.png?size=64")
        );
    }

    #[test]
    fn homepage_fallback_is_a_conventional_path_without_scraping() {
        let mut station = station();
        station.homepage_url = Some("https://radio.example.test/home?utm=ignored".into());
        assert_eq!(
            source_url(&station).as_deref(),
            Some("https://radio.example.test/favicon.ico")
        );
    }

    #[test]
    fn unsafe_urls_are_rejected() {
        let mut station = station();
        station.favicon_url = Some("file:///tmp/logo.png".into());
        assert!(source_url(&station).is_none());
        station.favicon_url = Some("https://user:secret@example.test/logo.png".into());
        assert!(source_url(&station).is_none());
    }

    #[test]
    fn cache_stem_cannot_escape_cache_directory() {
        let path = PathBuf::from("cache").join(format!("{}.bin", cache_stem(&station())));
        assert_eq!(path.parent(), Some(Path::new("cache")));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("v1-")
        );
        assert!(!path.to_string_lossy().contains(".."));
    }

    #[test]
    fn cache_round_trip_is_deterministic_without_network() {
        let dir = std::env::temp_dir().join(format!("rockcast-icon-test-{}", std::process::id()));
        let path = dir.join("v1-test.bin");
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255];
        write_cache(&path, "https://example.test/favicon.ico", 2, 1, &rgba).unwrap();
        let image = read_cache(&path, "https://example.test/favicon.ico", "v1-test")
            .unwrap()
            .unwrap();
        assert_eq!((image.width, image.height, image.rgba), (2, 1, rgba));
        let _ = fs::remove_dir_all(dir);
    }
}
