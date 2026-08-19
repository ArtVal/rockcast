//! Volume scaling for local speakers and Cast receivers.

pub(crate) const VOLUME_CAST_SCALE: f32 = 0.5;

pub(crate) fn cast_volume(percent: u8) -> f32 {
    (f32::from(percent) / 100.0 * VOLUME_CAST_SCALE).clamp(0.0, VOLUME_CAST_SCALE)
}

pub(crate) fn local_volume(percent: u8) -> f32 {
    (f32::from(percent) / 100.0).clamp(0.0, 1.0)
}
