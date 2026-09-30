//! Station catalog list view, search bar, filters, table headers, and station rows.

mod filter_chips;
mod missing_row;
mod search_bar;
mod station_row;
mod table_header;

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    Ui, Vec2,
};

use super::super::{RockCastApp, StationFilterMode, theme::*};
use missing_row::{MissingRowAction, StationRow, draw_missing_favourite_row};
use station_row::RowAction;

impl RockCastApp {
    pub(in crate::app) fn draw_station_list(&mut self, ui: &mut Ui, list_h: f32) {
        let t = self.lang.t();

        let search_outcome = self.draw_search_bar(ui);
        let filter_search_requested = self.draw_filter_chips(ui);
        ui.add_space(6.0);

        if search_outcome.return_home {
            self.station_search.clear();
            self.selected_genre = None;
            self.filter_mode = StationFilterMode::All;
            self.refresh_stations();
        } else if search_outcome.search_requested || filter_search_requested {
            let query = self.global_station_query();
            if query.is_empty() {
                self.refresh_stations();
            } else {
                self.search_stations(query);
            }
        }

        let mut should_play = false;
        let mut clicked_station: Option<usize> = None;
        let mut toggle_fav: Option<crate::stations::Station> = None;
        let mut unfav_missing: Option<(String, String)> = None;
        let mut resolve_missing: Option<(String, String)> = None;

        let loading_stations = t.loading_stations;
        let list_empty = t.list_empty;
        let is_loading = self.loading_stations;

        panel(ui, |ui| {
            let layout = self.calculate_table_layout(ui, list_h);
            self.draw_table_header(ui, &layout, &t);

            let rows = self.build_station_rows();
            let show_footer = self.filter_mode == StationFilterMode::All
                && (self.loading_more_stations
                    || self.loading_more_error.is_some()
                    || (!self.station_has_more && rows.len() >= 20));
            let total_rows = rows.len() + if show_footer { 1 } else { 0 };
            let mut need_load_more = false;

            let mut scroll_area = egui::ScrollArea::vertical()
                .id_salt("stations_scroll")
                .auto_shrink([false, false])
                .max_height(layout.scroll_h)
                .min_scrolled_height(layout.scroll_h);

            if let Some(target_idx) = self.scroll_to_station {
                let target_y = target_idx as f32 * ROW_H;
                let centered_y = (target_y - layout.scroll_h * 0.5 + ROW_H * 0.5).max(0.0);
                scroll_area = scroll_area.vertical_scroll_offset(centered_y);
            }

            let scroll_output = scroll_area.show_rows(ui, ROW_H, total_rows, |ui, row_range| {
                let row_w = ui.available_width();

                if rows.is_empty() {
                    let empty_text = match self.filter_mode {
                        StationFilterMode::Favourites => {
                            "В избранном пока нет станций. Нажмите ★ рядом со станцией в списке."
                        }
                        StationFilterMode::History => "История прослушиваний пока пуста.",
                        StationFilterMode::All => {
                            if is_loading {
                                loading_stations
                            } else {
                                list_empty
                            }
                        }
                    };
                    ui.allocate_ui_with_layout(
                        Vec2::new(layout.full_w, layout.scroll_h - 8.0),
                        Layout::centered_and_justified(egui::Direction::TopDown),
                        |ui| {
                            ui.label(RichText::new(empty_text).color(MUTED).size(14.0));
                        },
                    );
                    return;
                }

                for row_pos in row_range.clone() {
                    if row_pos < rows.len() {
                        match &rows[row_pos] {
                            StationRow::Missing { station_id, name } => {
                                match draw_missing_favourite_row(
                                    ui,
                                    &layout,
                                    row_w,
                                    row_pos,
                                    station_id,
                                    name,
                                    self.resolving_stations.contains(station_id),
                                ) {
                                    MissingRowAction::Unfavourite => {
                                        unfav_missing = Some((station_id.clone(), name.clone()));
                                    }
                                    MissingRowAction::Play => {
                                        resolve_missing = Some((station_id.clone(), name.clone()));
                                    }
                                    MissingRowAction::None => {}
                                }
                            }
                            StationRow::Loaded(index) => {
                                match self.draw_loaded_station_row(
                                    ui, &layout, row_w, row_pos, *index,
                                ) {
                                    RowAction::ToggleFav(st) => toggle_fav = Some(st),
                                    RowAction::Select { index, play } => {
                                        clicked_station = Some(index);
                                        if play {
                                            should_play = true;
                                        }
                                    }
                                    RowAction::Stop { index } => {
                                        clicked_station = Some(index);
                                        self.stop();
                                    }
                                    RowAction::None => {}
                                }
                            }
                        }
                    } else {
                        self.draw_stations_footer_row(ui, row_w);
                    }
                }

                if self.filter_mode == StationFilterMode::All
                    && row_range.end >= rows.len().saturating_sub(3)
                    && self.station_has_more
                    && !self.loading_more_stations
                    && !self.loading_stations
                    && self.loading_more_error.is_none()
                {
                    need_load_more = true;
                }
            });

            if need_load_more {
                self.load_more_stations();
            }

            if scroll_output.state.offset.y > 400.0 {
                let btn_w = 92.0;
                let btn_h = 30.0;
                let btn_rect = Rect::from_min_size(
                    Pos2::new(
                        scroll_output.inner_rect.right() - btn_w - 20.0,
                        scroll_output.inner_rect.bottom() - btn_h - 14.0,
                    ),
                    Vec2::new(btn_w, btn_h),
                );
                let btn_resp = ui.interact(btn_rect, ui.id().with("to_top_btn"), Sense::click());
                let hovered = btn_resp.hovered();
                let bg = if hovered {
                    ACCENT
                } else {
                    Color32::from_rgba_premultiplied(32, 28, 25, 235)
                };
                let border = if hovered { Color32::WHITE } else { BORDER };
                let tint = if hovered { Color32::WHITE } else { ACCENT };

                ui.painter().rect_filled(btn_rect, CornerRadius::same(15), bg);
                ui.painter().rect_stroke(
                    btn_rect,
                    CornerRadius::same(15),
                    Stroke::new(1.0, border),
                    StrokeKind::Inside,
                );

                let icon_size = 14.0;
                let icon_rect = Rect::from_min_size(
                    Pos2::new(btn_rect.min.x + 12.0, btn_rect.center().y - icon_size * 0.5),
                    Vec2::splat(icon_size),
                );
                ui.painter().image(
                    self.app_icons.arrow_up.id(),
                    icon_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    tint,
                );
                ui.painter().text(
                    Pos2::new(btn_rect.min.x + 32.0, btn_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    "Наверх",
                    FontId::proportional(FS_SMALL),
                    tint,
                );

                if btn_resp.clicked() {
                    self.scroll_to_station = Some(0);
                    ui.ctx().request_repaint();
                }
            }

            self.draw_column_guides(ui, &layout);
        });

        if let Some(st) = toggle_fav {
            self.toggle_station_favourite(&st);
        }
        if let Some((station_id, name)) = unfav_missing {
            self.remove_missing_favourite(&station_id, &name);
        }
        if let Some((station_id, name)) = resolve_missing {
            self.begin_resolve_missing_station(&station_id, &name);
        }

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
        self.scroll_to_station = None;
        if should_play {
            log::info!("station double-click → play()");
            self.play();
        }
    }

    fn draw_stations_footer_row(&mut self, ui: &mut Ui, row_w: f32) {
        let (row_rect, _resp) = ui.allocate_exact_size(Vec2::new(row_w, ROW_H), Sense::hover());
        let center = row_rect.center();

        if self.loading_more_stations {
            let time = ui.input(|i| i.time) as f32;
            let pulse = ((time * 3.5).sin() * 0.4 + 0.6).clamp(0.2, 1.0);
            let color = ACCENT.gamma_multiply(pulse);
            ui.painter().text(
                center,
                egui::Align2::CENTER_CENTER,
                "Загрузка станций…",
                FontId::proportional(FS_ROW),
                color,
            );
            ui.ctx().request_repaint();
        } else if self.loading_more_error.is_some() {
            let btn_w = 260.0;
            let btn_h = 28.0;
            let btn_rect = Rect::from_center_size(center, Vec2::new(btn_w, btn_h));
            let btn_resp = ui.interact(btn_rect, ui.id().with("footer_retry_btn"), Sense::click());
            let hovered = btn_resp.hovered();
            let bg = if hovered { ACCENT } else { PANEL_2 };
            let fg = if hovered { Color32::WHITE } else { ACCENT };
            ui.painter().rect_filled(btn_rect, CornerRadius::same(6), bg);
            ui.painter().rect_stroke(
                btn_rect,
                CornerRadius::same(6),
                Stroke::new(1.0, BORDER),
                StrokeKind::Inside,
            );
            ui.painter().text(
                center,
                egui::Align2::CENTER_CENTER,
                "Ошибка сети. Загрузить ещё",
                FontId::proportional(FS_SMALL),
                fg,
            );
            if btn_resp.clicked() {
                self.loading_more_error = None;
                self.load_more_stations();
            }
        } else if !self.station_has_more && self.stations.len() >= 20 {
            let count = self.stations.len();
            ui.painter().text(
                center,
                egui::Align2::CENTER_CENTER,
                format!("Показаны все станции ({count})"),
                FontId::proportional(FS_SMALL),
                MUTED,
            );
        }
    }

    fn global_station_query(&self) -> String {
        let mut terms = self.station_search.trim().to_owned();
        if let Some(genre) = &self.selected_genre
            && !terms.to_lowercase().contains(&genre.to_lowercase())
        {
            if !terms.is_empty() {
                terms.push(' ');
            }
            terms.push_str(genre);
        }
        terms
    }
}
