//! Rendering and interaction handling for loaded station rows.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::app::{
    RockCastApp,
    theme::{
        ACCENT, BORDER, FG, FS_ROW, GOLD_STAR, MUTED, PANEL_2, ROW_H, ROW_PLAY_BTN,
        format_country_code, station_color, truncate,
    },
};

use super::table_header::TableLayout;

pub(super) enum RowAction {
    None,
    ToggleFav(crate::stations::Station),
    Select { index: usize, play: bool },
    Stop { index: usize },
}

impl RockCastApp {
    pub(super) fn draw_loaded_station_row(
        &mut self,
        ui: &mut Ui,
        layout: &TableLayout,
        row_w: f32,
        row_pos: usize,
        index: usize,
    ) -> RowAction {
        let st = &self.stations[index];
        let selected = self.selected_station == Some(index);
        let meta = [
            if st.bitrate > 0 {
                format!("{}k", st.bitrate)
            } else {
                String::new()
            },
            st.codec.clone(),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
        let tags_limit = ((layout.tags_w / 6.8) as usize).clamp(18, 72);
        let meta_limit = ((layout.meta_w / 6.4) as usize).clamp(10, 28);
        let tags = truncate(&st.tags, tags_limit);
        let country = format_country_code(&st.country);
        let meta = truncate(&meta, meta_limit);

        let (row_rect, resp) = ui.allocate_exact_size(Vec2::new(row_w, ROW_H), Sense::click());
        if self.scroll_to_station == Some(index) {
            ui.scroll_to_rect(row_rect, Some(Align::Center));
        }

        let mut star_clicked = false;
        let mut play_action: Option<RowAction> = None;

        if ui.is_rect_visible(row_rect) {
            let is_current = self.station_now == st.name;
            let is_playing_this = is_current && self.playing;
            let bg = if is_playing_this {
                Color32::from_rgba_unmultiplied(229, 96, 32, 46)
            } else if selected {
                Color32::from_rgba_unmultiplied(229, 96, 32, 42)
            } else if resp.hovered() {
                Color32::from_rgb(0x27, 0x1f, 0x1a)
            } else if row_pos % 2 == 1 {
                Color32::from_rgb(0x1e, 0x17, 0x12)
            } else {
                Color32::from_rgb(0x18, 0x13, 0x0f)
            };
            ui.painter().rect_filled(row_rect, CornerRadius::same(4), bg);

            if selected || is_current {
                let bar_rect = Rect::from_min_max(
                    Pos2::new(row_rect.left(), row_rect.top()),
                    Pos2::new(row_rect.left() + 3.5, row_rect.bottom()),
                );
                ui.painter().rect_filled(bar_rect, CornerRadius::ZERO, ACCENT);
            }

            let text_color = if is_current { ACCENT } else { FG };
            let muted_color = MUTED;
            let y = row_rect.center().y;

            // 1-Click Interactive Star Button
            let star_rect =
                Rect::from_center_size(Pos2::new(row_rect.left() + 16.0, y), Vec2::splat(22.0));
            let star_resp = ui.interact(
                star_rect,
                ui.id().with(("station_star", &st.id)),
                Sense::click(),
            );
            let is_fav = self.is_station_favourite(&st.id);
            let (star_icon, star_color) = if is_fav {
                ("★", GOLD_STAR)
            } else if star_resp.hovered() {
                ("☆", FG)
            } else {
                ("☆", MUTED.gamma_multiply(0.4))
            };
            ui.painter().text(
                star_rect.center(),
                egui::Align2::CENTER_CENTER,
                star_icon,
                egui::FontId::proportional(14.0),
                star_color,
            );
            if star_resp.clicked() {
                star_clicked = true;
            }

            // Station Logo or 2-letter monogram tile with rock brand palette
            let icon_rect =
                Rect::from_center_size(Pos2::new(row_rect.left() + 40.0, y), Vec2::splat(26.0));
            let mut has_image = false;
            if let Some(source) =
                crate::station_icons::source_url(st, Some(self.rockserver.base_url()))
            {
                let request_key = crate::station_icons::request_key(st, &source);
                if let Some(texture) = self.station_icons.get(&request_key) {
                    has_image = true;
                    ui.painter().image(
                        texture.id(),
                        icon_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            }
            if !has_image {
                let chars: Vec<char> = st.name.chars().filter(|c| c.is_alphanumeric()).collect();
                let initial = if chars.len() >= 2 {
                    format!("{}{}", chars[0], chars[1]).to_uppercase()
                } else if let Some(c) = chars.first() {
                    c.to_uppercase().to_string()
                } else {
                    "?".to_string()
                };
                let monogram_fill = station_color(&st.name);
                ui.painter().rect_filled(
                    icon_rect,
                    CornerRadius::same(5),
                    monogram_fill,
                );
                ui.painter().rect_stroke(
                    icon_rect,
                    CornerRadius::same(5),
                    Stroke::new(1.0, Color32::from_rgb(0x4a, 0x38, 0x2c)),
                    StrokeKind::Inside,
                );
                ui.painter().text(
                    icon_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    initial,
                    egui::FontId::proportional(11.0),
                    Color32::from_rgb(0xec, 0xe4, 0xdc),
                );
            }

            let name_start_x = row_rect.left() + layout.col_name_x;
            // Reserve room for the animated equalizer so it never
            // bleeds into the tags column.
            let eq_room = if is_playing_this { 34.0 } else { 0.0 };
            let name_limit = (((layout.name_w - eq_room) / 7.5) as usize).clamp(16, 64);
            let name_text = truncate(&st.name, name_limit);
            let name_size = ui.painter().text(
                Pos2::new(name_start_x, y),
                egui::Align2::LEFT_CENTER,
                &name_text,
                FontId::proportional(FS_ROW),
                text_color,
            );
            if is_playing_this {
                // Animated 4-bar "now playing" equalizer right after the name.
                let now = ui.input(|i| i.time) as f32;
                let eq_x = name_start_x + name_size.width() + 10.0;
                let (bar_w, gap, max_h) = (3.0, 2.5, 14.0);
                for bar in 0..4 {
                    let phase = now * (4.2 + bar as f32 * 1.35) + bar as f32 * 1.1;
                    let h = max_h * (0.35 + 0.65 * (0.5 + 0.5 * phase.sin()));
                    let x0 = eq_x + bar as f32 * (bar_w + gap);
                    ui.painter().rect_filled(
                        Rect::from_min_max(
                            Pos2::new(x0, y - h * 0.5),
                            Pos2::new(x0 + bar_w, y + h * 0.5),
                        ),
                        CornerRadius::same(1),
                        ACCENT,
                    );
                }
            }

            ui.painter().text(
                Pos2::new(row_rect.left() + layout.col_tags_x, y),
                egui::Align2::LEFT_CENTER,
                tags,
                egui::FontId::proportional(12.0),
                muted_color,
            );
            ui.painter().text(
                Pos2::new(row_rect.left() + layout.col_meta_x, y),
                egui::Align2::LEFT_CENTER,
                meta,
                egui::FontId::proportional(11.5),
                muted_color,
            );

            // Country badge
            let country_rect = Rect::from_center_size(
                Pos2::new(row_rect.left() + layout.col_country_x + layout.country_w * 0.5, y),
                Vec2::new(26.0, 16.0),
            );
            ui.painter().rect_filled(country_rect, CornerRadius::same(3), PANEL_2);
            ui.painter().rect_stroke(
                country_rect,
                CornerRadius::same(3),
                Stroke::new(1.0, BORDER),
                StrokeKind::Inside,
            );
            ui.painter().text(
                country_rect.center(),
                egui::Align2::CENTER_CENTER,
                &country,
                egui::FontId::proportional(10.0),
                muted_color,
            );

            // 1-Click Play/Pause Button on each row — always visible
            let play_btn_rect = Rect::from_center_size(
                Pos2::new(row_rect.left() + layout.play_center_x, y),
                Vec2::splat(ROW_PLAY_BTN),
            );
            let play_resp = ui.interact(
                play_btn_rect,
                ui.id().with(("row_play", &st.id)),
                Sense::click(),
            );
            let hovered_or_playing = is_playing_this || play_resp.hovered();
            let (play_fill, play_fg, play_stroke) = if hovered_or_playing {
                (ACCENT, Color32::WHITE, Stroke::NONE)
            } else {
                (
                    PANEL_2,
                    Color32::from_rgb(0xd5, 0xca, 0xc0),
                    Stroke::new(1.0, BORDER),
                )
            };
            ui.painter()
                .circle_filled(play_btn_rect.center(), ROW_PLAY_BTN * 0.5, play_fill);
            if play_stroke.width > 0.0 {
                ui.painter().circle_stroke(
                    play_btn_rect.center(),
                    ROW_PLAY_BTN * 0.5,
                    play_stroke,
                );
            }
            let play_icon = if is_playing_this { "⏸" } else { "▶" };
            let x_off = if is_playing_this { 0.0 } else { 1.0 };
            ui.painter().text(
                Pos2::new(
                    play_btn_rect.center().x + x_off,
                    play_btn_rect.center().y,
                ),
                egui::Align2::CENTER_CENTER,
                play_icon,
                FontId::proportional(13.0),
                play_fg,
            );
            if play_resp.clicked() {
                play_action = Some(if is_playing_this {
                    RowAction::Stop { index }
                } else {
                    RowAction::Select { index, play: true }
                });
            }
        }

        if star_clicked {
            return RowAction::ToggleFav(st.clone());
        }
        if let Some(action) = play_action {
            return action;
        }
        if resp.double_clicked() {
            return RowAction::Select { index, play: true };
        }
        if resp.clicked() {
            return RowAction::Select { index, play: false };
        }

        RowAction::None
    }
}
