//! Unified live internet-radio decode: one path for local speakers and relay.

mod local;
mod open;
mod relay;
mod symphonia;

pub use local::run_live_decode_f32;
pub use relay::run_live_decode_relay_pcm;
