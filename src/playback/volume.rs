//! Volume scaling for local speakers and Cast receivers.

pub(crate) fn local_volume(percent: u8) -> f32 {
    (f32::from(percent) / 100.0).clamp(0.0, 1.0)
}
