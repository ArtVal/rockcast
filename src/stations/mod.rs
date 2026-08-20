//! Rock / metal radio stations: local catalog + Radio Browser API.

mod catalog;
mod radio_browser;

use crate::i18n::{self, Lang};

pub use catalog::{infer_codec, parse_stations_txt};
pub use radio_browser::enrich_stations;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Station {
    pub name: String,
    pub url: String,
    pub tags: String,
    pub country: String,
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
        Station {
            name: "test".into(),
            url: "https://example.test/stream".into(),
            tags: String::new(),
            country: String::new(),
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
    fn aac_url_overrides_missing_codec() {
        let mut s = station("");
        s.url = "http://example.test/live.aac".into();
        assert_eq!(s.content_type(), "audio/aac");
    }
}
