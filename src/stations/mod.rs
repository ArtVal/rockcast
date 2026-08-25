//! Rock / metal radio stations: local catalog + Radio Browser API.

mod catalog;
mod radio_browser;

use crate::i18n::{self, Lang};

pub(crate) use catalog::catalog_resolver;
pub use catalog::{infer_codec, parse_stations_txt};
pub use radio_browser::enrich_stations;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Station {
    /// Stable schema-v1 ID. Transitional/ad-hoc sources use a deterministic local ID.
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub url: String,
    pub tags: String,
    pub country: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub homepage_url: Option<String>,
    #[serde(default)]
    pub favicon_url: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub legacy_ids: Vec<String>,
    pub bitrate: u32,
    pub codec: String,
    /// Includes primary and alternatives; url/codec/bitrate remain the primary playback fields.
    #[serde(default)]
    pub streams: Vec<StationStream>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StationStream {
    pub id: String,
    pub url: String,
    pub codec: String,
    pub bitrate: u32,
    pub primary: bool,
}

impl Station {
    pub fn from_primary(
        id: String,
        name: String,
        url: String,
        tags: String,
        country: String,
        bitrate: u32,
        codec: String,
    ) -> Self {
        Self {
            id,
            name,
            url: url.clone(),
            tags,
            country,
            language: None,
            homepage_url: None,
            favicon_url: None,
            aliases: Vec::new(),
            legacy_ids: Vec::new(),
            bitrate,
            codec: codec.clone(),
            streams: vec![StationStream {
                id: "main".into(),
                url,
                codec,
                bitrate,
                primary: true,
            }],
        }
    }

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

/// Instant local list (no network).
pub fn load_catalog(lang: Lang) -> (Vec<Station>, String) {
    let ordered = catalog::order_stations(catalog::catalog_stations());
    let n = ordered.len();
    (ordered, i18n::fmt1(lang.t().local_catalog, n))
}

#[cfg(test)]
mod tests {
    use super::Station;

    fn station(codec: &str) -> Station {
        Station::from_primary(
            "test".into(),
            "test".into(),
            "https://example.test/stream".into(),
            String::new(),
            String::new(),
            128,
            codec.into(),
        )
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
    fn aac_url_overrides_missing_codec() {
        let mut s = station("");
        s.url = "http://example.test/live.aac".into();
        assert_eq!(s.content_type(), "audio/aac");
    }
}
