//! Symphonia format probes — AAC streams must not compete with the MP3 demuxer.

use std::sync::OnceLock;

use symphonia::core::probe::Probe;

/// Probe that only accepts ADTS AAC (SomaFM and similar `audio/aac` ICY streams).
pub fn adts_only() -> &'static Probe {
    static PROBE: OnceLock<Probe> = OnceLock::new();
    PROBE.get_or_init(|| {
        let mut probe = Probe::default();
        probe.register_all::<symphonia::default::formats::AdtsReader>();
        probe
    })
}
