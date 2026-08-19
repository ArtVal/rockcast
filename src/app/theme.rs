//! egui theme tokens and layout helpers.

use std::time::Duration;

use eframe::egui::{self, Color32, CornerRadius, Frame, Ui};

pub(crate) const BG: Color32 = Color32::from_rgb(0x1a, 0x14, 0x10);
pub(crate) const PANEL: Color32 = Color32::from_rgb(0x24, 0x1c, 0x16);
pub(crate) const PANEL_2: Color32 = Color32::from_rgb(0x2e, 0x24, 0x1c);
pub(crate) const FG: Color32 = Color32::from_rgb(0xe8, 0xdc, 0xc8);
pub(crate) const ACCENT: Color32 = Color32::from_rgb(0xc4, 0x5c, 0x26);
pub(crate) const MUTED: Color32 = Color32::from_rgb(0x9a, 0x8b, 0x78);
pub(crate) const BAR_DIM: Color32 = Color32::from_rgb(0x5a, 0x40, 0x30);
pub(crate) const ROW_H: f32 = 24.0;
pub(crate) const COL_RESIZE_HIT_W: f32 = 10.0;
pub(crate) const NAME_COL_MIN: f32 = 120.0;
pub(crate) const NAME_COL_MAX: f32 = 360.0;
pub(crate) const TAGS_COL_MIN: f32 = 110.0;
pub(crate) const META_COL_MIN: f32 = 88.0;
/// EQ bar animation target rate (~20 FPS).
pub(crate) const EQ_REPAINT_INTERVAL: Duration = Duration::from_millis(50);
/// Background polling while playback/loading is active.
pub(crate) const UI_SLOW_REPAINT_INTERVAL: Duration = Duration::from_millis(120);

pub(crate) fn panel(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
        Frame::new()
            .fill(PANEL)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::same(12))
            .show(ui, add);
    }

pub(crate) fn truncate(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
