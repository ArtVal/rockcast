//! Bounded RockServer HTTP search client; microphone streaming is layered separately.
use crate::stations::Station;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Queries RockServer and converts its public station DTOs into playback stations.
pub fn search(
    base_url: &str,
    bearer_token: &str,
    query: &str,
    locale: &str,
) -> Result<Vec<Station>, String> {
    let base = base_url.trim().trim_end_matches('/');
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err("RockServer URL must start with http:// or https://".into());
    }
    let bearer_token = bearer_token.trim();
    if bearer_token.is_empty() {
        return Err("RockServer token is required".into());
    }
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| "RockServer HTTP client failed".to_owned())?
        .post(format!("{base}/api/v1/search"))
        .bearer_auth(bearer_token)
        .json(&SearchRequest {
            query,
            locale,
            limit: 50,
        })
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
        .map(|item| Station {
            name: item.name,
            url: item.stream_url,
            tags: item.tags.join(", "),
            country: item.country_code.unwrap_or_default(),
            bitrate: item.bitrate_kbps.unwrap_or(0),
            codec: item.codec.unwrap_or_default(),
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
    name: String,
    stream_url: String,
    #[serde(default)]
    tags: Vec<String>,
    bitrate_kbps: Option<u32>,
    codec: Option<String>,
    country_code: Option<String>,
}
