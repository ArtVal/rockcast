//! egui panel widgets.

use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2,
};

use super::super::RockCastApp;
use super::super::theme::*;

impl RockCastApp {
    pub(in crate::app) fn draw_station_list(&mut self, ui: &mut Ui, list_h: f32) {
        let t = self.lang.t();
        ui.horizontal(|ui| {
            ui.label(RichText::new(t.stations).color(FG).size(15.0).strong());
            if !self.source.is_empty() {
                ui.label(
                    RichText::new(format!("· {}", self.source))
                        .color(MUTED)
                        .size(12.0),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let btn = egui::Button::new(RichText::new(t.refresh).color(FG))
                    .fill(PANEL_2)
                    .min_size(Vec2::new(92.0, 26.0));
                if ui.add_enabled(!self.loading_stations, btn).clicked() {
                    self.refresh_stations();
                }
            });
        });
        ui.add_space(4.0);

        let mut should_play = false;
        let mut clicked_station: Option<usize> = None;
        let scroll_h = (list_h - 72.0).max(100.0);
        let col_station = t.col_station;
        let col_tags = t.col_tags;
        let col_country = t.col_country;
        let col_bitrate = t.col_bitrate;
        let loading_stations = t.loading_stations;
        let list_empty = t.list_empty;
        let is_loading = self.loading_stations;
        panel(ui, |ui| {
            let full_w = ui.available_width();
            let reserve = 24.0;
            let available = (full_w - reserve)
                .max(NAME_COL_MIN + TAGS_COL_MIN + COUNTRY_COL_MIN + META_COL_MIN);
            let longest_name = self
                .stations
                .iter()
                .map(|s| s.name.chars().count())
                .max()
                .unwrap_or(0);
            let default_name_w: f32 = if longest_name > 34 {
                260.0
            } else if longest_name > 24 {
                220.0
            } else {
                180.0
            };
            let default_name_w = default_name_w
                .clamp(NAME_COL_MIN, NAME_COL_MAX)
                .min(available - TAGS_COL_MIN - COUNTRY_COL_MIN - META_COL_MIN);
            let default_tags_w = (available * 0.44)
                .clamp(180.0, 340.0)
                .min(available - default_name_w - COUNTRY_COL_MIN - META_COL_MIN);
            let country_w = COUNTRY_COL_MIN;

            let mut name_w = self.station_name_col_w.unwrap_or(default_name_w);
            let mut tags_w = self.station_tags_col_w.unwrap_or(default_tags_w);
            name_w = name_w.clamp(NAME_COL_MIN, NAME_COL_MAX);
            tags_w = tags_w.clamp(
                TAGS_COL_MIN,
                (available - name_w - country_w - META_COL_MIN).max(TAGS_COL_MIN),
            );
            let mut meta_w = available - name_w - tags_w - country_w;
            if meta_w < META_COL_MIN {
                let deficit = META_COL_MIN - meta_w;
                let tags_shrink = (tags_w - TAGS_COL_MIN).min(deficit);
                tags_w -= tags_shrink;
                let remaining = deficit - tags_shrink;
                if remaining > 0.0 {
                    name_w = (name_w - remaining).max(NAME_COL_MIN);
                }
                meta_w = available - name_w - tags_w - country_w;
            }

            let col_name_x = 8.0;
            let col_tags_x = 8.0 + name_w;
            let col_country_x = 8.0 + name_w + tags_w;
            let col_meta_x = col_country_x + country_w;
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
                    .clamp(NAME_COL_MIN, NAME_COL_MAX)
                    .min(available - TAGS_COL_MIN - COUNTRY_COL_MIN - META_COL_MIN);
                self.station_name_col_w = Some(new_name);
                name_w = new_name;
                tags_w = tags_w.clamp(
                    TAGS_COL_MIN,
                    (available - name_w - country_w - META_COL_MIN).max(TAGS_COL_MIN),
                );
                meta_w = available - name_w - tags_w - country_w;
            }
            if right_resp.dragged() {
                let new_tags = (tags_w + right_resp.drag_delta().x).clamp(
                    TAGS_COL_MIN,
                    (available - name_w - country_w - META_COL_MIN).max(TAGS_COL_MIN),
                );
                self.station_tags_col_w = Some(new_tags);
                tags_w = new_tags;
                meta_w = available - name_w - tags_w - country_w;
            }

            {
                let (head_rect, _) =
                    ui.allocate_exact_size(Vec2::new(full_w, 20.0), Sense::hover());
                let y = head_rect.center().y;
                ui.painter().text(
                    Pos2::new(head_rect.left() + col_name_x, y),
                    egui::Align2::LEFT_CENTER,
                    col_station,
                    egui::FontId::proportional(12.0),
                    MUTED,
                );
                ui.painter().text(
                    Pos2::new(head_rect.left() + col_tags_x, y),
                    egui::Align2::LEFT_CENTER,
                    col_tags,
                    egui::FontId::proportional(12.0),
                    MUTED,
                );
                ui.painter().text(
                    Pos2::new(head_rect.left() + col_meta_x, y),
                    egui::Align2::LEFT_CENTER,
                    col_bitrate,
                    egui::FontId::proportional(12.0),
                    MUTED,
                );
                ui.painter().text(
                    Pos2::new(head_rect.left() + col_country_x, y),
                    egui::Align2::LEFT_CENTER,
                    col_country,
                    egui::FontId::proportional(12.0),
                    MUTED,
                );
            }
            ui.add_space(4.0);
            let sep_y = ui.cursor().top();
            ui.painter().hline(
                ui.max_rect().x_range(),
                sep_y,
                Stroke::new(1.0, Color32::from_rgb(0x3a, 0x2e, 0x24)),
            );
            ui.add_space(6.0);

            egui::ScrollArea::vertical()
                .id_salt("stations_scroll")
                .auto_shrink([false, false])
                .max_height(scroll_h)
                .min_scrolled_height(scroll_h)
                .show(ui, |ui| {
                    ui.set_min_width(full_w);
                    if self.stations.is_empty() {
                        ui.allocate_ui_with_layout(
                            Vec2::new(full_w, scroll_h - 8.0),
                            Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                let msg = if is_loading {
                                    loading_stations
                                } else {
                                    list_empty
                                };
                                ui.label(RichText::new(msg).color(MUTED).size(14.0));
                            },
                        );
                        return;
                    }

                    for (i, st) in self.stations.iter().enumerate() {
                        let selected = self.selected_station == Some(i);
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
                        let tags_limit = ((tags_w / 6.8) as usize).clamp(18, 72);
                        let meta_limit = ((meta_w / 6.4) as usize).clamp(10, 28);
                        let tags = truncate(&st.tags, tags_limit);
                        let country =
                            truncate(&st.country, ((country_w / 7.0) as usize).clamp(10, 18));
                        let meta = truncate(&meta, meta_limit);

                        let (row_rect, resp) =
                            ui.allocate_exact_size(Vec2::new(full_w, ROW_H), Sense::click());
                        if self.scroll_to_station == Some(i) {
                            ui.scroll_to_rect(row_rect, Some(Align::Center));
                        }
                        if ui.is_rect_visible(row_rect) {
                            let bg = if selected {
                                ACCENT
                            } else if resp.hovered() {
                                PANEL_2
                            } else if i % 2 == 1 {
                                Color32::from_rgb(0x20, 0x18, 0x14)
                            } else {
                                Color32::TRANSPARENT
                            };
                            ui.painter()
                                .rect_filled(row_rect, CornerRadius::same(4), bg);

                            let text_color = if selected { BG } else { FG };
                            let muted_color = if selected {
                                Color32::from_rgb(0x3a, 0x28, 0x1c)
                            } else {
                                MUTED
                            };
                            let y = row_rect.center().y;

                            ui.painter().text(
                                Pos2::new(row_rect.left() + col_name_x, y),
                                egui::Align2::LEFT_CENTER,
                                truncate(&st.name, ((name_w / 7.5) as usize).clamp(16, 64)),
                                egui::FontId::proportional(13.5),
                                text_color,
                            );
                            ui.painter().text(
                                Pos2::new(row_rect.left() + col_tags_x, y),
                                egui::Align2::LEFT_CENTER,
                                tags,
                                egui::FontId::proportional(12.5),
                                muted_color,
                            );
                            ui.painter().text(
                                Pos2::new(row_rect.left() + col_meta_x, y),
                                egui::Align2::LEFT_CENTER,
                                meta,
                                egui::FontId::proportional(12.5),
                                muted_color,
                            );
                            ui.painter().text(
                                Pos2::new(row_rect.left() + col_country_x, y),
                                egui::Align2::LEFT_CENTER,
                                country,
                                egui::FontId::proportional(12.5),
                                muted_color,
                            );
                        }

                        if resp.clicked() {
                            clicked_station = Some(i);
                        }
                        if resp.double_clicked() {
                            clicked_station = Some(i);
                            should_play = true;
                        }
                    }
                });

            let guide_color = if left_resp.dragged() || right_resp.dragged() {
                ACCENT.gamma_multiply(0.9)
            } else {
                Color32::from_rgba_unmultiplied(255, 255, 255, 18)
            };
            let guide_top = ui.min_rect().top() + 2.0;
            let guide_bottom = ui.min_rect().top() + scroll_h + 24.0;
            ui.painter().vline(
                ui.min_rect().left() + col_tags_x,
                guide_top..=guide_bottom,
                Stroke::new(1.0, guide_color),
            );
            ui.painter().vline(
                ui.min_rect().left() + col_meta_x,
                guide_top..=guide_bottom,
                Stroke::new(1.0, guide_color),
            );
        });

        if let Some(i) = clicked_station {
            let prev = self.selected_station;
            self.selected_station = Some(i);
            self.scroll_to_station = Some(i);
            if let Some(s) = self.stations.get(i) {
                log::info!(
                    "station selected: idx={i} (was {prev:?}) name='{}' url={} auto_play={should_play}",
                    s.name,
                    s.url
                );
            }
            self.mark_settings_dirty();
        }
        if clicked_station.is_some() || self.scroll_to_station == self.selected_station {
            self.scroll_to_station = None;
        }
        if should_play {
            log::info!("station double-click → play()");
            self.play();
        }
    }
}
