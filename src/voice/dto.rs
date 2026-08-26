//! RockServer voice WebSocket DTOs.

use crate::stations::Station;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum VoiceEvent {
    Ready {},
    Transcript {
        transcript: String,
        is_final: bool,
    },
    Result {
        transcript: String,
        normalized_query: NormalizedQueryDto,
        #[serde(default)]
        stations: Vec<StationDto>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Deserialize)]
pub(super) struct NormalizedQueryDto {
    pub action: VoiceAction,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum VoiceAction {
    Play,
    Show,
}

#[derive(Debug, Deserialize)]
pub(super) struct StationDto {
    pub id: String,
    pub name: String,
    pub stream_url: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub bitrate_kbps: Option<u32>,
    pub codec: Option<String>,
    pub country_code: Option<String>,
    #[serde(default, alias = "homepageUrl")]
    pub homepage_url: Option<String>,
    #[serde(default, alias = "faviconUrl")]
    pub favicon_url: Option<String>,
    pub score: f64,
}

impl From<StationDto> for Station {
    fn from(v: StationDto) -> Self {
        let url = v.stream_url;
        let mut station = Self::from_primary(
            v.id,
            v.name,
            url,
            v.tags.join(", "),
            v.country_code.unwrap_or_default(),
            v.bitrate_kbps.unwrap_or(0),
            v.codec.unwrap_or_default(),
        );
        station.homepage_url = v.homepage_url;
        station.favicon_url = v.favicon_url;
        station
    }
}
