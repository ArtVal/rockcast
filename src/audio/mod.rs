//! Shared audio helpers: format sniffing, spectrum analysis, live decode.

pub mod decode;
pub mod format;
pub mod spectrum;

pub use format::{
    apply_format_hint, apply_hint, find_adts_sync, find_mp3_sync, infer_stream_format,
    is_mp3_stream, parse_stream_title, read_format_peek, PrefixedReader, StreamFormat,
};
pub use spectrum::{BandAnalyzer, LevelPublisher, SpectrumTap, BANDS, LEVEL_PUBLISH_INTERVAL};
