//! RockCast library (GUI binary is `main.rs`).

pub mod app;
pub mod audio;
pub mod profile;
pub mod cast;
pub mod i18n;
pub mod icy;
pub mod local;
pub mod net;
pub mod observers;
pub mod output;
pub mod playback;
pub mod playback_diag;
pub mod relay;
/// Optional RockServer search integration.
pub mod rockserver;
pub mod runtime;
pub mod settings;
pub mod spectrum;
pub mod stations;
pub mod telemetry;
/// Microphone capture and RockServer voice transport.
pub mod voice;
/// Embedded voice prompt playback (beep / "turning on" / "not found").
pub mod voice_prompts;
