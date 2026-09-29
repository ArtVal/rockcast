//! Column geometry layout, draggable resize handles, and table header drawing.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2,
};

use crate::{
    app::{
        RockCastApp,
        theme::{
            ACCENT, COL_RESIZE_HIT_W, COUNTRY_COL_W, FS_SMALL, META_COL_MIN,
            MUTED, NAME_COL_MAX, NAME_COL_MIN, PANEL_2, ROW_PAD_RIGHT, ROW_PLAY_BTN,
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
    pub(super) is_dragged: bool,
}

impl RockCastApp {
    pub(super) fn calculate_table_layout(&mut self, ui: &mut Ui, list_h: f32) -> TableLayout {
        let full_w = ui.available_width();
        let scroll_h = (list_h - 136.0).max(100.0);
        let col_name_x = 68.0;
        let play_col_w = ROW_PLAY_BTN + ROW_PAD_RIGHT + 10.0;
        let country_w = COUNTRY_COL_W;
        let meta_w = META_COL_MIN;
        let fixed_w = col_name_x + meta_w + country_w + play_col_w;
        let name_tags_w = (full_w - fixed_w).max(NAME_COL_MIN + TAGS_COL_MIN);

        let default_name_w: f32 = (name_tags_w * 0.44)
            .clamp(NAME_COL_MIN, NAME_COL_MAX)
            .min(name_tags_w - TAGS_COL_MIN);
        let mut name_w = self
            .station_name_col_w
            .unwrap_or(default_name_w)
            .clamp(NAME_COL_MIN, NAME_COL_MAX.min(name_tags_w - TAGS_COL_MIN));
        let mut tags_w = self
            .station_tags_col_w
            .unwrap_or(name_tags_w - name_w)
            .clamp(TAGS_COL_MIN, name_tags_w - name_w);
        if name_w + tags_w > name_tags_w {
            tags_w = (name_tags_w - name_w).max(TAGS_COL_MIN);
        }

        let col_tags_x = col_name_x + name_w;
        let col_meta_x = col_tags_x + tags_w;
        let col_country_x = col_meta_x + meta_w;
        let top = ui.cursor().top();

        let left_handle = Rect::from_min_max(
            Pos2::new(
                ui.min_rect().left() + col_tags_x - COL_RESIZE_HIT_W * 0.5,
                top,
            ),
            Pos2::new(
                ui.min_rect().left() + col_tags_x + COL_RESIZE_HIT_W * 0.5,
                top + scroll_h + 28.0,
            ),
        );
        let right_handle = Rect::from_min_max(
            Pos2::new(
                ui.min_rect().left() + col_meta_x - COL_RESIZE_HIT_W * 0.5,
                top,
            ),
            Pos2::new(
                ui.min_rect().left() + col_meta_x + COL_RESIZE_HIT_W * 0.5,
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
            let new_name = (name_w + left_resp.drag_delta().x)
                .clamp(NAME_COL_MIN, NAME_COL_MAX.min(name_tags_w - TAGS_COL_MIN));
            self.station_name_col_w = Some(new_name);
            name_w = new_name;
            tags_w = tags_w.clamp(TAGS_COL_MIN, (name_tags_w - name_w).max(TAGS_COL_MIN));
            self.station_tags_col_w = Some(tags_w);
        }
        if right_resp.dragged() {
            let new_tags = (tags_w + right_resp.drag_delta().x)
                .clamp(TAGS_COL_MIN, (name_tags_w - name_w).max(TAGS_COL_MIN));
            self.station_tags_col_w = Some(new_tags);
            tags_w = new_tags;
        }

        let is_dragged = left_resp.dragged() || right_resp.dragged();

        TableLayout {
            full_w,
            scroll_h,
            col_name_x,
            name_w,
            tags_w,
            meta_w,
            country_w,
            col_tags_x,
            col_meta_x,
            col_country_x,
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
        let bar_reserve = ui.spacing().scroll.floating_allocated_width;
        ui.painter().text(
            Pos2::new(
                head_rect.left() + layout.full_w
                    - bar_reserve
                    - ROW_PAD_RIGHT
                    - ROW_PLAY_BTN * 0.5,
                y,
            ),
            egui::Align2::CENTER_CENTER,
            "▶",
            FontId::proportional(11.0),
            MUTED,
        );

        ui.add_space(4.0);
        let sep_y = ui.cursor().top();
        ui.painter().hline(
            ui.max_rect().x_range(),
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
