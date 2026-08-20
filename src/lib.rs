//! RockCast library (GUI binary is `main.rs`).

pub mod app;
pub mod audio;
pub mod cast;
pub mod i18n;
pub mod local;
pub mod net;
pub mod observers;
pub mod output;
pub mod playback;
pub mod playback_diag;
pub mod profile;
pub mod relay;
/// Optional RockServer search integration.
pub mod rockserver;
pub mod runtime;
pub mod settings;
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
