//! Column geometry layout, draggable resize handles, and table header drawing.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2,
};

use crate::{
    app::{
        RockCastApp,
        theme::{
            ACCENT, COL_RESIZE_HIT_W, COUNTRY_COL_W, FS_SMALL, META_COL_MIN,
            MUTED, NAME_COL_MAX, NAME_COL_MIN, PANEL_2, ROW_PLAY_BTN,
            TAGS_COL_MIN,
        },
    },
    i18n,
};

pub(super) struct TableLayout {
    pub(super) full_w: f32,
    pub(super) scroll_h: f32,
    pub(super) col_name_x: f32,
    pub(super) name_w: f32,
    pub(super) tags_w: f32,
    pub(super) meta_w: f32,
    pub(super) country_w: f32,
    pub(super) col_tags_x: f32,
    pub(super) col_meta_x: f32,
    pub(super) col_country_x: f32,
    pub(super) play_center_x: f32,
    pub(super) is_dragged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TableGeometry {
    pub(super) full_w: f32,
    pub(super) row_w: f32,
    pub(super) col_name_x: f32,
    pub(super) name_w: f32,
    pub(super) tags_w: f32,
    pub(super) meta_w: f32,
    pub(super) country_w: f32,
    pub(super) col_tags_x: f32,
    pub(super) col_meta_x: f32,
    pub(super) col_country_x: f32,
    pub(super) col_play_x: f32,
    pub(super) play_w: f32,
    pub(super) play_center_x: f32,
    pub(super) name_tags_w: f32,
}

pub(super) fn compute_table_geometry(
    available_w: f32,
    screen_w: f32,
    scrollbar_reserve: f32,
    saved_name_w: Option<f32>,
    saved_tags_w: Option<f32>,
) -> TableGeometry {
    let max_visible_w = (screen_w - 56.0).max(300.0);
    let full_w = available_w.min(max_visible_w);
    let row_w = (full_w - scrollbar_reserve).max(300.0);

    let col_name_x = 68.0;
    let play_w = ROW_PLAY_BTN + 20.0;
    let country_w = COUNTRY_COL_W;
    let meta_w = META_COL_MIN;
    let fixed_w = col_name_x + meta_w + country_w + play_w;
    let name_tags_w = (row_w - fixed_w).max(NAME_COL_MIN + TAGS_COL_MIN);

    let default_name_w: f32 = (name_tags_w * 0.44)
        .clamp(NAME_COL_MIN, NAME_COL_MAX)
        .min(name_tags_w - TAGS_COL_MIN);
    let max_name = (name_tags_w - TAGS_COL_MIN).max(NAME_COL_MIN);
    let mut name_w = saved_name_w
        .unwrap_or(default_name_w)
        .clamp(NAME_COL_MIN, NAME_COL_MAX.min(max_name));
    let mut tags_w = (name_tags_w - name_w).max(TAGS_COL_MIN);
    if let Some(saved_tags) = saved_tags_w {
        let max_tags = (name_tags_w - NAME_COL_MIN).max(TAGS_COL_MIN);
        tags_w = saved_tags.clamp(TAGS_COL_MIN, max_tags);
        name_w = (name_tags_w - tags_w).max(NAME_COL_MIN);
    }

    let col_tags_x = col_name_x + name_w;
    let col_meta_x = col_tags_x + tags_w;
    let col_country_x = col_meta_x + meta_w;
    let col_play_x = col_country_x + country_w;
    let play_center_x = col_play_x + play_w * 0.5;

    TableGeometry {
        full_w,
        row_w,
        col_name_x,
        name_w,
        tags_w,
        meta_w,
        country_w,
        col_tags_x,
        col_meta_x,
        col_country_x,
        col_play_x,
        play_w,
        play_center_x,
        name_tags_w,
    }
}

impl RockCastApp {
    pub(super) fn calculate_table_layout(&mut self, ui: &mut Ui, list_h: f32) -> TableLayout {
        let scroll_h = (ui.available_height() - 44.0).clamp(100.0, (list_h - 136.0).max(100.0));
        let scrollbar_reserve = (ui.spacing().scroll.bar_width
            + ui.spacing().scroll.bar_inner_margin
            + ui.spacing().scroll.bar_outer_margin)
            .max(14.0);
        let geom = compute_table_geometry(
            ui.available_width(),
            ui.ctx().viewport_rect().width(),
            scrollbar_reserve,
            self.station_name_col_w,
            self.station_tags_col_w,
        );

        let top = ui.cursor().top();

        let left_handle = Rect::from_min_max(
            Pos2::new(
                ui.min_rect().left() + geom.col_tags_x - COL_RESIZE_HIT_W * 0.5,
                top,
            ),
            Pos2::new(
                ui.min_rect().left() + geom.col_tags_x + COL_RESIZE_HIT_W * 0.5,
                top + scroll_h + 28.0,
            ),
        );
        let right_handle = Rect::from_min_max(
            Pos2::new(
                ui.min_rect().left() + geom.col_meta_x - COL_RESIZE_HIT_W * 0.5,
                top,
            ),
            Pos2::new(
                ui.min_rect().left() + geom.col_meta_x + COL_RESIZE_HIT_W * 0.5,
                top + scroll_h + 28.0,
            ),
        );
        let left_resp = ui.interact(
            left_handle,
            ui.id().with("station_col_resize_left"),
            Sense::drag(),
        );
        let right_resp = ui.interact(
            right_handle,
            ui.id().with("station_col_resize_right"),
            Sense::drag(),
        );
        if left_resp.hovered()
            || left_resp.dragged()
            || right_resp.hovered()
            || right_resp.dragged()
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if left_resp.dragged() {
            let max_name = (geom.name_tags_w - TAGS_COL_MIN).max(NAME_COL_MIN);
            let new_name = (geom.name_w + left_resp.drag_delta().x)
                .clamp(NAME_COL_MIN, NAME_COL_MAX.min(max_name));
            self.station_name_col_w = Some(new_name);
            self.station_tags_col_w = Some((geom.name_tags_w - new_name).max(TAGS_COL_MIN));
        }
        if right_resp.dragged() {
            let max_tags = (geom.name_tags_w - NAME_COL_MIN).max(TAGS_COL_MIN);
            let new_tags = (geom.tags_w + right_resp.drag_delta().x)
                .clamp(TAGS_COL_MIN, max_tags);
            self.station_tags_col_w = Some(new_tags);
            self.station_name_col_w = Some((geom.name_tags_w - new_tags).max(NAME_COL_MIN));
        }

        let is_dragged = left_resp.dragged() || right_resp.dragged();

        TableLayout {
            full_w: geom.full_w,
            scroll_h,
            col_name_x: geom.col_name_x,
            name_w: geom.name_w,
            tags_w: geom.tags_w,
            meta_w: geom.meta_w,
            country_w: geom.country_w,
            col_tags_x: geom.col_tags_x,
            col_meta_x: geom.col_meta_x,
            col_country_x: geom.col_country_x,
            play_center_x: geom.play_center_x,
            is_dragged,
        }
    }

    pub(super) fn draw_table_header(&self, ui: &mut Ui, layout: &TableLayout, t: &i18n::Strings) {
        let (head_rect, _) =
            ui.allocate_exact_size(Vec2::new(layout.full_w, 28.0), Sense::hover());
        let y = head_rect.center().y;
        let header_font = FontId::proportional(FS_SMALL);
        ui.painter()
            .rect_filled(head_rect, CornerRadius::same(4), PANEL_2);
        ui.painter().text(
            Pos2::new(head_rect.left() + 16.0, y),
            egui::Align2::CENTER_CENTER,
            "★",
            FontId::proportional(12.0),
            MUTED,
        );
        ui.painter().text(
            Pos2::new(head_rect.left() + layout.col_name_x, y),
            egui::Align2::LEFT_CENTER,
            t.col_station,
            header_font.clone(),
            MUTED,
        );
        ui.painter().text(
            Pos2::new(head_rect.left() + layout.col_tags_x, y),
            egui::Align2::LEFT_CENTER,
            t.col_tags,
            header_font.clone(),
            MUTED,
        );
        ui.painter().text(
            Pos2::new(head_rect.left() + layout.col_meta_x, y),
            egui::Align2::LEFT_CENTER,
            t.col_bitrate,
            header_font.clone(),
            MUTED,
        );
        ui.painter().text(
            Pos2::new(head_rect.left() + layout.col_country_x + layout.country_w * 0.5, y),
            egui::Align2::CENTER_CENTER,
            t.col_country,
            header_font,
            MUTED,
        );
        ui.painter().text(
            Pos2::new(head_rect.left() + layout.play_center_x, y),
            egui::Align2::CENTER_CENTER,
            "▶",
            FontId::proportional(11.0),
            MUTED,
        );

        ui.add_space(4.0);
        let sep_y = ui.cursor().top();
        ui.painter().hline(
            head_rect.x_range(),
            sep_y,
            Stroke::new(1.0, Color32::from_rgb(0x3a, 0x2e, 0x24)),
        );
        ui.add_space(6.0);
    }

    pub(super) fn draw_column_guides(&self, ui: &mut Ui, layout: &TableLayout) {
        let guide_color = if layout.is_dragged {
            ACCENT.gamma_multiply(0.9)
        } else {
            Color32::from_rgba_unmultiplied(255, 255, 255, 18)
        };
        let guide_top = ui.min_rect().top() + 2.0;
        let guide_bottom = ui.min_rect().top() + layout.scroll_h + 24.0;
        ui.painter().vline(
            ui.min_rect().left() + layout.col_tags_x,
            guide_top..=guide_bottom,
            Stroke::new(1.0, guide_color),
        );
        ui.painter().vline(
            ui.min_rect().left() + layout.col_meta_x,
            guide_top..=guide_bottom,
            Stroke::new(1.0, guide_color),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_geometry_standard_window_fits_columns() {
        // Window 940px wide, unconstrained available_width might be 1400px (e.g. from wide filters)
        let geom = compute_table_geometry(1400.0, 940.0, 14.0, None, None);

        // full_w is clamped to 940.0 - 56.0 = 884.0
        assert_eq!(geom.full_w, 884.0);
        assert_eq!(geom.row_w, 870.0);

        // Total width of all columns must fit within row_w
        let table_right = geom.col_play_x + geom.play_w;
        assert!(
            table_right <= geom.row_w,
            "Table right edge ({}) must be <= row_w ({})",
            table_right,
            geom.row_w
        );

        // Column sequences are contiguous
        assert_eq!(geom.col_tags_x, geom.col_name_x + geom.name_w);
        assert_eq!(geom.col_meta_x, geom.col_tags_x + geom.tags_w);
        assert_eq!(geom.col_country_x, geom.col_meta_x + geom.meta_w);
        assert_eq!(geom.col_play_x, geom.col_country_x + geom.country_w);
    }

    #[test]
    fn table_geometry_min_window_fits_columns() {
        // Minimum window size is 820px wide
        let geom = compute_table_geometry(820.0, 820.0, 14.0, None, None);

        // full_w is clamped to 820.0 - 56.0 = 764.0
        assert_eq!(geom.full_w, 764.0);
        assert_eq!(geom.row_w, 750.0);

        let table_right = geom.col_play_x + geom.play_w;
        assert!(
            table_right <= geom.row_w,
            "Table right edge ({}) must be <= row_w ({})",
            table_right,
            geom.row_w
        );

        assert!(geom.name_w >= NAME_COL_MIN);
        assert!(geom.tags_w >= TAGS_COL_MIN);
    }

    #[test]
    fn table_geometry_respects_custom_column_widths_within_bounds() {
        // Test custom wide name column
        let geom = compute_table_geometry(884.0, 940.0, 14.0, Some(350.0), None);
        assert!(geom.name_w <= NAME_COL_MAX);
        assert!(geom.tags_w >= TAGS_COL_MIN);

        let table_right = geom.col_play_x + geom.play_w;
        assert!(table_right <= geom.row_w);
    }
}


