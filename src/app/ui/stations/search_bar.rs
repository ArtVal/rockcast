//! Search input bar, voice interaction button, catalog reset button, and voice status banner.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Frame, Layout, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, Ui, Vec2,
};

use crate::app::{
    RockCastApp,
    theme::{ACCENT, BORDER, FG, FS_BODY, MUTED, PANEL, PANEL_2},
};

#[derive(Default)]
pub(super) struct SearchBarOutcome {
    pub(super) search_requested: bool,
    pub(super) return_home: bool,
}

impl RockCastApp {
    pub(super) fn draw_search_bar(&mut self, ui: &mut Ui) -> SearchBarOutcome {
        let mut outcome = SearchBarOutcome::default();
        let is_recording = self.voice_recording.is_some();
        let is_busy = self.voice_busy;

        ui.horizontal(|ui| {
            let has_text = !self.station_search.trim().is_empty();
            let catalog_btn_w = 125.0;
            let frame_margin_x = 20.0;
            let search_box_w =
                (ui.available_width() - catalog_btn_w - 8.0 - frame_margin_x).max(180.0);

            // Cohesive search input container
            Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(10, 4))
                .show(ui, |ui| {
                    ui.set_width(search_box_w);
                    ui.horizontal(|ui| {
                        let (icon_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
                        ui.painter().image(
                            self.app_icons.search.id(),
                            icon_rect,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            MUTED,
                        );
                        let clear_btn_w = if has_text { 24.0 } else { 0.0 };
                        let voice_btn_w = 84.0;
                        let edit_w = (search_box_w - voice_btn_w - clear_btn_w - 28.0).max(80.0);

                        let response = ui.add_sized(
                            [edit_w, 24.0],
                            egui::TextEdit::singleline(&mut self.station_search)
                                .hint_text("Поиск станции, группы или стиля…")
                                .frame(false),
                        );
                        let enter = response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));

                        if has_text {
                            let (clr_rect, clr_resp) =
                                ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
                            let tint = if clr_resp.hovered() {
                                Color32::WHITE
                            } else {
                                MUTED
                            };
                            ui.painter().image(
                                self.app_icons.clear.id(),
                                clr_rect.shrink(5.0),
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                tint,
                            );
                            if clr_resp.clicked() {
                                self.station_search.clear();
                                outcome.search_requested = true;
                            }
                            let _ = clr_resp.on_hover_text("Очистить поиск / Clear search");
                        }

                        if enter && has_text {
                            outcome.search_requested = true;
                        }

                        let (voice_caption, voice_fill, voice_text_color) = if is_recording {
                            ("Стоп", Color32::from_rgb(0xdc, 0x26, 0x26), Color32::WHITE)
                        } else if is_busy {
                            ("Распознаю…", PANEL_2, MUTED)
                        } else if has_text {
                            ("Найти", ACCENT, Color32::WHITE)
                        } else {
                            ("Голос", PANEL_2, ACCENT)
                        };

                        let (voice_rect, voice_resp) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width().max(60.0), 24.0),
                            if !is_busy || is_recording {
                                Sense::click()
                            } else {
                                Sense::hover()
                            },
                        );
                        let hovered_active = voice_resp.hovered() && (!is_busy || is_recording);
                        let bg = if is_recording {
                            if hovered_active {
                                Color32::from_rgb(0xef, 0x44, 0x44)
                            } else {
                                voice_fill
                            }
                        } else if hovered_active {
                            Color32::from_rgb(0xf0, 0x70, 0x30)
                        } else {
                            voice_fill
                        };
                        ui.painter()
                            .rect_filled(voice_rect, CornerRadius::same(6), bg);
                        if !has_text || is_recording || is_busy {
                            ui.painter().rect_stroke(
                                voice_rect,
                                CornerRadius::same(6),
                                Stroke::new(1.0, BORDER),
                                StrokeKind::Inside,
                            );
                        }

                        if !is_recording && !is_busy && !has_text {
                            let mic_rect = Rect::from_center_size(
                                Pos2::new(voice_rect.left() + 16.0, voice_rect.center().y),
                                Vec2::splat(15.0),
                            );
                            ui.painter().image(
                                self.app_icons.mic.id(),
                                mic_rect,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                voice_text_color,
                            );
                            ui.painter().text(
                                Pos2::new(voice_rect.left() + 28.0, voice_rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                voice_caption,
                                FontId::proportional(FS_BODY),
                                voice_text_color,
                            );
                        } else {
                            ui.painter().text(
                                voice_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                voice_caption,
                                FontId::proportional(FS_BODY),
                                voice_text_color,
                            );
                        }

                        let clicked = voice_resp.clicked();
                        if clicked {
                            if is_recording {
                                self.stop_voice_recording();
                            } else if has_text {
                                outcome.search_requested = true;
                            } else if !is_busy {
                                self.start_voice();
                            }
                        } else if enter && !has_text && !is_busy && !is_recording {
                            self.start_voice();
                        }
                    });
                });

            // Quick catalog refresh button
            let cat_label = format!("Каталог ({})", self.stations.len());
            let cat_btn = egui::Button::new(RichText::new(cat_label).color(FG).size(12.0))
                .min_size(Vec2::new(catalog_btn_w, 32.0))
                .corner_radius(CornerRadius::same(8))
                .stroke(Stroke::new(1.0, BORDER))
                .fill(PANEL);
            if ui.add(cat_btn).clicked() {
                outcome.return_home = true;
            }
        });

        if self.voice_recording.is_some() || self.voice_busy {
            ui.add_space(2.0);
            Frame::new()
                .fill(Color32::from_rgba_unmultiplied(229, 96, 32, 26))
                .stroke(Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(229, 96, 32, 75),
                ))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (dot_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                        ui.painter().circle_filled(dot_rect.center(), 3.5, ACCENT);
                        let text = if self.voice_recording.is_some() {
                            "Слушаю вас… Назовите станцию или жанр (запись завершится сама)"
                        } else {
                            "Распознаю голос и ищу подходящую станцию в каталоге…"
                        };
                        ui.label(RichText::new(text).color(FG).size(12.0).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .button(RichText::new("✕ Отмена").size(11.0).color(MUTED))
                                .clicked()
                            {
                                self.cancel_voice();
                            }
                        });
                    });
                });
        } else if let Some(voice_q) = self.voice_search_query.clone() {
            ui.add_space(2.0);
            Frame::new()
                .fill(Color32::from_rgba_unmultiplied(229, 96, 32, 22))
                .stroke(Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(229, 96, 32, 60),
                ))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 5))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (mic_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                        ui.painter().image(
                            self.app_icons.mic.id(),
                            mic_rect,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            ACCENT,
                        );
                        let text = format!("Распознано голосом: «{voice_q}»");
                        ui.label(RichText::new(text).color(FG).size(12.0).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .button(RichText::new("✕ Сбросить").size(11.0).color(MUTED))
                                .on_hover_text("Очистить голосовой фильтр и вернуться к каталогу")
                                .clicked()
                            {
                                self.voice_search_query = None;
                                outcome.return_home = true;
                            }
                        });
                    });
                });
        }

        outcome
    }
}
