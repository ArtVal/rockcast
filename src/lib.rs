//! RockCast library (GUI binary is `main.rs`). See the [source map](ARCHITECTURE.md).

pub mod app;
pub mod audio;
pub mod cast;
pub(crate) mod device_control;
pub mod i18n;
pub mod local;
pub mod net;
pub mod observers;
pub mod output;
pub mod personal_data;
/// Client side of the RM-012-B favourites/history server sync.
pub mod personal_sync;
pub mod playback;
pub mod playback_diag;
pub mod profile;
pub mod relay;
/// Optional RockServer search integration.
pub mod rockserver;
pub mod runtime;
pub mod session;
pub mod settings;
pub mod station_icons;
pub mod stations;
pub mod telemetry;
/// Microphone capture and RockServer voice transport.
pub mod voice;
/// Embedded voice prompt playback (beep / "turning on" / "not found").
pub mod voice_prompts;

// Back-compat for tests/examples that import `rockcast::spectrum` or `rockcast::icy`.
pub mod icy {
    pub use crate::observers::IcyWatcher;
}
pub mod spectrum {
    pub use crate::observers::spectrum::*;
}
pub use observers::spectrum::{BANDS, SpectrumAnalyzer};
