//! Rendering and interaction handling for loaded station rows with TableBuilder.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2,
};
use egui_extras::TableRow;

use crate::app::{
    RockCastApp,
    theme::{
        ACCENT, BORDER, FG, FS_BODY, FS_ROW, FS_SMALL, GOLD_STAR, MUTED, PANEL_2, ROW_PLAY_BTN,
        format_country_code, station_color,
    },
};

pub(super) enum RowAction {
    None,
    ToggleFav(crate::stations::Station),
    Select { index: usize, play: bool },
    Stop { index: usize },
}

impl RockCastApp {
    pub(super) fn draw_loaded_station_row(
        &self,
        row: &mut TableRow<'_, '_>,
        row_pos: usize,
        index: usize,
        table_x_range: egui::Rangef,
    ) -> RowAction {
        let st = &self.stations[index];
        let selected = self.selected_station == Some(index);
        let is_playing_this = self.playing
            && if let Some(play_id) = self.playback_station_id.as_deref() {
                st.id == play_id
            } else if let Some(play_url) = self.playing_url.as_deref() {
                st.url == play_url
            } else if !self.station_now.is_empty() && self.station_now != "—" {
                self.stations.iter().position(|s| s.name == self.station_now) == Some(index)
            } else {
                selected
            };
        let is_current = if self.playing {
            is_playing_this
        } else {
            selected
        };

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
        let country = format_country_code(&st.country);

        let text_color = if is_current { ACCENT } else { FG };

        let mut star_clicked = false;
        let mut play_action: Option<RowAction> = None;
        let mut row_clicked = false;
        let mut row_double_clicked = false;

        // Background color helper and row click interaction for each cell
        let draw_cell_bg = |ui: &mut egui::Ui, col_idx: usize| -> (bool, bool) {
            let cell_rect = ui.max_rect();
            let is_row_hovered = ui.ctx().pointer_latest_pos().is_some_and(|p| {
                cell_rect.y_range().contains(p.y) && table_x_range.contains(p.x)
            });
            let bg = if is_playing_this {
                Color32::from_rgba_unmultiplied(229, 96, 32, 46)
            } else if selected {
                Color32::from_rgba_unmultiplied(229, 96, 32, 42)
            } else if is_row_hovered {
                Color32::from_rgb(0x27, 0x1f, 0x1a)
            } else if row_pos % 2 == 1 {
                Color32::from_rgb(0x1e, 0x17, 0x12)
            } else {
                Color32::from_rgb(0x18, 0x13, 0x0f)
            };
            let corner = if col_idx == 0 {
                CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 }
            } else if col_idx == 5 {
                CornerRadius { nw: 0, sw: 0, ne: 4, se: 4 }
            } else {
                CornerRadius::ZERO
            };
            ui.painter().rect_filled(cell_rect, corner, bg);

            if selected {
                ui.painter().hline(
                    cell_rect.x_range(),
                    cell_rect.top(),
                    Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(229, 96, 32, 120)),
                );
                ui.painter().hline(
                    cell_rect.x_range(),
                    cell_rect.bottom() - 1.0,
                    Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(229, 96, 32, 120)),
                );
            }

            let cell_resp = ui.interact(
                cell_rect,
                ui.id().with(("row_cell_click", index, col_idx)),
                Sense::click(),
            );
            (cell_resp.clicked(), cell_resp.double_clicked())
        };

        // --- Col 0: Star + Monogram / Station Logo (68px fixed) ---
        row.col(|ui| {
            let (c, dc) = draw_cell_bg(ui, 0);
            if dc { row_double_clicked = true; } else if c { row_clicked = true; }

            if selected || is_current {
                let bar_rect = Rect::from_min_size(
                    ui.max_rect().left_top(),
                    Vec2::new(3.5, ui.max_rect().height()),
                );
                ui.painter().rect_filled(bar_rect, CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 }, ACCENT);
            }

            ui.horizontal_centered(|ui| {
                ui.add_space(8.0);

                // 1-Click Interactive Star Button
                let is_fav = self.is_station_favourite(&st.id);
                let (star_icon, star_color) = if is_fav {
                    ("★", GOLD_STAR)
                } else {
                    ("☆", MUTED.gamma_multiply(0.5))
                };
                let star_btn = egui::Button::new(
                    RichText::new(star_icon).size(14.0).color(star_color)
                ).frame(false);
                if ui.add(star_btn).clicked() {
                    star_clicked = true;
                }

                ui.add_space(4.0);

                // Monogram or Logo
                let icon_size = Vec2::splat(26.0);
                let mut has_image = false;
                if let Some(source) =
                    crate::station_icons::source_url(st, Some(self.rockserver.base_url()))
                {
                    let request_key = crate::station_icons::request_key(st, &source);
                    if let Some(texture) = self.station_icons.get(&request_key) {
                        has_image = true;
                        ui.image((texture.id(), icon_size));
                    }
                }
                if !has_image {
                    let (icon_rect, _) = ui.allocate_exact_size(icon_size, Sense::hover());
                    let chars: Vec<char> = st.name.chars().filter(|c| c.is_alphanumeric()).collect();
                    let initial = if chars.len() >= 2 {
                        format!("{}{}", chars[0], chars[1]).to_uppercase()
                    } else if let Some(c) = chars.first() {
                        c.to_uppercase().to_string()
                    } else {
                        "?".to_string()
                    };
                    let monogram_fill = station_color(&st.name);
                    ui.painter().rect_filled(icon_rect, CornerRadius::same(5), monogram_fill);
                    ui.painter().rect_stroke(
                        icon_rect,
                        CornerRadius::same(5),
                        Stroke::new(1.0_f32, Color32::from_rgb(0x4a, 0x38, 0x2c)),
                        StrokeKind::Inside,
                    );
                    ui.painter().text(
                        icon_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        initial,
                        FontId::proportional(11.0),
                        Color32::from_rgb(0xec, 0xe4, 0xdc),
                    );
                }
            });
        });

        // --- Col 1: Station Name + Animated Equalizer (Resizable) ---
        row.col(|ui| {
            let (c, dc) = draw_cell_bg(ui, 1);
            if dc { row_double_clicked = true; } else if c { row_clicked = true; }

            ui.horizontal_centered(|ui| {
                ui.add_space(4.0);

                let eq_w = if is_playing_this { 34.0 } else { 0.0 };
                let max_text_w = (ui.available_width() - eq_w).max(20.0);

                let label = egui::Label::new(
                    RichText::new(&st.name).color(text_color).size(FS_ROW)
                )
                .sense(Sense::click())
                .truncate();
                let resp = ui
                    .add_sized([max_text_w, ui.available_height()], label)
                    .on_hover_text(&st.name);
                if resp.double_clicked() {
                    row_double_clicked = true;
                } else if resp.clicked() {
                    row_clicked = true;
                }

                if is_playing_this {
                    ui.add_space(4.0);
                    let (eq_rect, _) = ui.allocate_exact_size(Vec2::new(22.0, 14.0), Sense::hover());
                    let cy = eq_rect.center().y;
                    let now = ui.input(|i| i.time) as f32;
                    let (bar_w, gap, max_h) = (3.0, 2.5, 14.0);
                    for bar in 0..4 {
                        let phase = now * (4.2 + bar as f32 * 1.35) + bar as f32 * 1.1;
                        let h = max_h * (0.35 + 0.65 * (0.5 + 0.5 * phase.sin()));
                        let x0 = eq_rect.left() + bar as f32 * (bar_w + gap);
                        ui.painter().rect_filled(
                            Rect::from_min_max(
                                Pos2::new(x0, cy - h * 0.5),
                                Pos2::new(x0 + bar_w, cy + h * 0.5),
                            ),
                            CornerRadius::same(1),
                            ACCENT,
                        );
                    }
                }
            });
        });

        // --- Col 2: Tags (Remainder) ---
        row.col(|ui| {
            let (c, dc) = draw_cell_bg(ui, 2);
            if dc { row_double_clicked = true; } else if c { row_clicked = true; }

            ui.horizontal_centered(|ui| {
                ui.add_space(4.0);

                let label = egui::Label::new(
                    RichText::new(&st.tags).color(MUTED).size(FS_BODY)
                )
                .sense(Sense::click())
                .truncate();
                let resp = ui.add(label);
                if resp.double_clicked() {
                    row_double_clicked = true;
                } else if resp.clicked() {
                    row_clicked = true;
                }
                resp.on_hover_text(&st.tags);
            });
        });

        // --- Col 3: Bitrate & Codec (80px) ---
        row.col(|ui| {
            let (c, dc) = draw_cell_bg(ui, 3);
            if dc { row_double_clicked = true; } else if c { row_clicked = true; }

            ui.horizontal_centered(|ui| {
                ui.add_space(4.0);

                let label = egui::Label::new(
                    RichText::new(&meta).color(MUTED).size(FS_SMALL)
                )
                .sense(Sense::click())
                .truncate();
                let resp = ui.add(label);
                if resp.double_clicked() {
                    row_double_clicked = true;
                } else if resp.clicked() {
                    row_clicked = true;
                }
            });
        });

        // --- Col 4: Country Badge (44px fixed) ---
        row.col(|ui| {
            let (c, dc) = draw_cell_bg(ui, 4);
            if dc { row_double_clicked = true; } else if c { row_clicked = true; }

            ui.centered_and_justified(|ui| {
                let (country_rect, country_resp) = ui.allocate_exact_size(Vec2::new(26.0, 16.0), Sense::click());
                if country_resp.double_clicked() {
                    row_double_clicked = true;
                } else if country_resp.clicked() {
                    row_clicked = true;
                }
                ui.painter().rect_filled(country_rect, CornerRadius::same(3), PANEL_2);
                ui.painter().rect_stroke(
                    country_rect,
                    CornerRadius::same(3),
                    Stroke::new(1.0_f32, BORDER),
                    StrokeKind::Inside,
                );
                ui.painter().text(
                    country_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &country,
                    FontId::proportional(10.0),
                    MUTED,
                );
            });
        });

        // --- Col 5: 1-Click Play/Pause Button (46px fixed) ---
        row.col(|ui| {
            let (c, dc) = draw_cell_bg(ui, 5);
            if dc { row_double_clicked = true; } else if c { row_clicked = true; }

            ui.centered_and_justified(|ui| {
                let (play_btn_rect, play_resp) = ui.allocate_exact_size(Vec2::splat(ROW_PLAY_BTN), Sense::click());
                let hovered_or_playing = is_playing_this || play_resp.hovered();
                let (play_fill, play_fg, play_stroke) = if hovered_or_playing {
                    (ACCENT, Color32::WHITE, Stroke::NONE)
                } else {
                    (
                        PANEL_2,
                        Color32::from_rgb(0xd5, 0xca, 0xc0),
                        Stroke::new(1.0_f32, BORDER),
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
            });
        });

        if star_clicked {
            return RowAction::ToggleFav(st.clone());
        }
        if let Some(action) = play_action {
            return action;
        }

        let resp = row.response();
        if row_double_clicked || resp.double_clicked() {
            return RowAction::Select { index, play: true };
        }
        if row_clicked || resp.clicked() {
            return RowAction::Select { index, play: false };
        }

        RowAction::None
    }
}
