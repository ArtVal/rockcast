//! egui panel widgets.

use eframe::egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::super::RockCastApp;
use super::super::theme::*;

impl RockCastApp {
    /// Draws the spectrum bars. No background box: the bars sit directly on
    /// the deck panel and span the full given height. Clicking toggles the
    /// spectrum on/off (state is visible: accent bars vs dim stubs).
    pub(in crate::app) fn draw_eq(&mut self, ui: &mut Ui, size: Vec2) {
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let painter = ui.painter_at(rect);
        let n = self.eq_levels.len();
        let gap = 3.0;
        let bar_w = ((rect.width() - gap * (n as f32 + 1.0)) / n as f32).max(2.0);
        let active = self.eq_enabled && self.playing;
        for (i, level) in self.eq_levels.iter().enumerate() {
            let h = (rect.height() - 4.0) * level.clamp(0.06, 1.0);
            let x0 = rect.left() + gap + i as f32 * (bar_w + gap);
            let y1 = rect.bottom() - 2.0;
            let y0 = y1 - h;
            let color = if active { ACCENT } else { BAR_DIM };
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x0 + bar_w, y1)),
                CornerRadius::same(2),
                color,
            );
            let peak = self.eq_peaks[i].clamp(0.06, 1.0);
            let peak_y = y1 - (rect.height() - 4.0) * peak;
            painter.line_segment(
                [Pos2::new(x0, peak_y), Pos2::new(x0 + bar_w, peak_y)],
                Stroke::new(1.5_f32, if active { Color32::WHITE } else { BAR_DIM }),
            );
        }
        if resp.clicked() {
            self.set_eq_enabled(!self.eq_enabled);
        }
        resp.on_hover_text(self.lang.t().spectrum_hint);
    }
}
