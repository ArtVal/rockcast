//! Station catalog list view, search bar, filters, table headers, and station rows.

mod filter_chips;
mod missing_row;
mod search_bar;
mod station_row;
mod table_header;

use eframe::egui::{self, Layout, RichText, Ui, Vec2};

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

            egui::ScrollArea::vertical()
                .id_salt("stations_scroll")
                .auto_shrink([false, false])
                .max_height(layout.scroll_h)
                .min_scrolled_height(layout.scroll_h)
                .show(ui, |ui| {
                    let row_w = ui.available_width();
                    let rows = self.build_station_rows();

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

                    for (row_pos, row) in rows.iter().enumerate() {
                        match row {
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
                    }
                });

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
        if clicked_station.is_some() || self.scroll_to_station == self.selected_station {
            self.scroll_to_station = None;
        }
        if should_play {
            log::info!("station double-click → play()");
            self.play();
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
