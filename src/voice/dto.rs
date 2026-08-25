//! RockServer voice WebSocket DTOs.

use crate::stations::Station;
use serde::Deserialize;

#[derive(Deserialize)]
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

#[derive(Deserialize)]
pub(super) struct NormalizedQueryDto {
    pub action: VoiceAction,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum VoiceAction {
    Play,
    Show,
}

#[derive(Deserialize)]
pub(super) struct StationDto {
    pub name: String,
    pub stream_url: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub bitrate_kbps: Option<u32>,
    pub codec: Option<String>,
    pub country_code: Option<String>,
    pub score: f64,
}

impl From<StationDto> for Station {
    fn from(v: StationDto) -> Self {
        let url = v.stream_url;
        Self::from_primary(
            format!(
                "rockserver-{}",
                url.bytes().fold(0_u64, |hash, byte| hash
                    .wrapping_mul(109)
                    .wrapping_add(byte as u64))
            ),
            v.name,
            url,
            v.tags.join(", "),
            v.country_code.unwrap_or_default(),
            v.bitrate_kbps.unwrap_or(0),
            v.codec.unwrap_or_default(),
        )
    }
}
