//! RockServer voice WebSocket DTOs.

use crate::stations::Station;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum VoiceCommandStatus {
    Succeeded,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum VoiceStreamErrorCode {
    ProtocolError,
    ValidationFailed,
    SpeechProviderUnavailable,
    SpeechProviderError,
    SpeechTimeout,
    SpeechNotRecognized,
    VoiceTimeout,
    AudioChunkInvalid,
    AudioTooLarge,
    Cancelled,
    IntentResolutionFailed,
    UnsupportedIntent,
    ClarificationRequired,
    StationNotFound,
    SearchTimeout,
    SearchUnavailable,
    TargetOffline,
    CapabilityNotSupported,
    Forbidden,
    InvalidPayload,
    CommandTimeout,
    DuplicateCommand,
    TooManyInFlight,
    PersistenceUnavailable,
    InternalError,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum VoiceEvent {
    Ready {},
    Transcript {
        transcript: String,
        is_final: bool,
    },
    Result {
        #[serde(default)]
        request_id: Option<String>,
        #[serde(default)]
        status: Option<VoiceCommandStatus>,
        #[serde(default)]
        transcript: Option<String>,
        #[serde(default)]
        normalized_query: Option<NormalizedQueryDto>,
        #[serde(default)]
        stations: Vec<StationDto>,
    },
    Error {
        #[serde(default)]
        code: Option<VoiceStreamErrorCode>,
        message: String,
        #[allow(dead_code)]
        #[serde(default)]
        request_id: Option<String>,
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
