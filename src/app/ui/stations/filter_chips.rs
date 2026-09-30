//! Filter chips for All, Favourites, History, and Genre selection.

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, Ui};

use crate::app::{
    RockCastApp, StationFilterMode,
    theme::{ACCENT, BORDER, FG, GOLD_STAR, MUTED, PANEL, PANEL_2},
};

const GENRES: [&str; 7] = [
    "Classic Rock",
    "Metal",
    "Hard Rock",
    "Alternative",
    "Punk",
    "Progressive",
    "Indie",
];

impl RockCastApp {
    pub(super) fn draw_filter_chips(&mut self, ui: &mut Ui) -> bool {
        let mut search_requested = false;

        ui.horizontal_wrapped(|ui| {
            let fav_count = self.personal_data.as_ref().map_or(0, |store| {
                store.favourites().len()
                    + store
                        .profile()
                        .unresolved_references
                        .iter()
                        .filter(|entry| entry.source_kind == "favourite")
                        .count()
            });
            let hist_count = self.personal_data.as_ref().map_or(0, |store| {
                store.history().len()
                    + store
                        .profile()
                        .unresolved_references
                        .iter()
                        .filter(|entry| entry.source_kind == "history")
                        .count()
            });

            let total_stations = self.stations.len();
            let matching_count = (0..total_stations)
                .filter(|&i| self.station_matches_facets(i))
                .count();
            let has_facets = self.selected_country.is_some() || self.selected_min_bitrate.is_some();

            // Filter chips: All, Favourites, History, Genres
            let all_selected = self.filter_mode == StationFilterMode::All
                && self.selected_genre.is_none();
            let all_label = if has_facets {
                format!("Все ({matching_count})")
            } else {
                match self.station_search_total {
                    Some(total) if total > total_stations => {
                        format!("Все ({total_stations} из {total})")
                    }
                    Some(total) => format!("Все ({total})"),
                    None => format!("Все ({total_stations})"),
                }
            };
            let all_btn = egui::Button::new(
                RichText::new(all_label)
                    .color(if all_selected { Color32::WHITE } else { MUTED })
                    .strong(),
            )
            .corner_radius(CornerRadius::same(12))
            .stroke(if all_selected {
                Stroke::NONE
            } else {
                Stroke::new(1.0, BORDER)
            })
            .fill(if all_selected { ACCENT } else { PANEL });
            if ui.add(all_btn).clicked() {
                let had_genre = self.selected_genre.is_some();
                self.filter_mode = StationFilterMode::All;
                self.selected_genre = None;
                if had_genre {
                    search_requested = true;
                }
            }

            let fav_selected = self.filter_mode == StationFilterMode::Favourites;
            let fav_btn = egui::Button::new(
                RichText::new(format!("★ Избранное {fav_count}"))
                    .color(if fav_selected {
                        Color32::WHITE
                    } else {
                        GOLD_STAR
                    })
                    .strong(),
            )
            .corner_radius(CornerRadius::same(12))
            .stroke(if fav_selected {
                Stroke::NONE
            } else {
                Stroke::new(1.0, BORDER)
            })
            .fill(if fav_selected { ACCENT } else { PANEL });
            if ui.add(fav_btn).clicked() {
                self.filter_mode = if fav_selected {
                    StationFilterMode::All
                } else {
                    StationFilterMode::Favourites
                };
            }

            let hist_selected = self.filter_mode == StationFilterMode::History;
            let hist_btn = egui::Button::new(
                RichText::new(format!("История {hist_count}"))
                    .color(if hist_selected { Color32::WHITE } else { FG })
                    .strong(),
            )
            .corner_radius(CornerRadius::same(12))
            .stroke(if hist_selected {
                Stroke::NONE
            } else {
                Stroke::new(1.0, BORDER)
            })
            .fill(if hist_selected { ACCENT } else { PANEL });
            if ui.add(hist_btn).clicked() {
                self.filter_mode = if hist_selected {
                    StationFilterMode::All
                } else {
                    StationFilterMode::History
                };
            }

            if hist_selected
                && hist_count > 0
                && ui
                    .button(RichText::new("✕ Очистить").size(11.0).color(MUTED))
                    .on_hover_text("Очистить историю воспроизведения на этом устройстве")
                    .clicked()
                && let Some(store) = self.personal_data.as_mut()
            {
                let _ = store.clear_history();
                self.schedule_personal_sync();
            }

            ui.add_space(4.0);
            ui.label(RichText::new("|").color(BORDER));
            ui.add_space(4.0);

            for genre in GENRES {
                let selected = self.filter_mode == StationFilterMode::All
                    && self.selected_genre.as_deref() == Some(genre);
                let genre_btn = egui::Button::new(
                    RichText::new(genre)
                        .color(if selected { Color32::WHITE } else { MUTED })
                        .size(12.0)
                        .strong(),
                )
                .corner_radius(CornerRadius::same(12))
                .stroke(if selected {
                    Stroke::NONE
                } else {
                    Stroke::new(1.0, BORDER)
                })
                .fill(if selected { ACCENT } else { PANEL });
                if ui.add(genre_btn).clicked() {
                    self.filter_mode = StationFilterMode::All;
                    self.selected_genre = (!selected).then(|| genre.to_owned());
                    search_requested = true;
                }
            }

            ui.add_space(4.0);
            ui.label(RichText::new("|").color(BORDER));
            ui.add_space(4.0);

            // Country facet ComboBox
            let country_text = match &self.selected_country {
                Some(c) => {
                    let c_matches = self
                        .stations
                        .iter()
                        .filter(|s| s.country.eq_ignore_ascii_case(c))
                        .count();
                    format!("Страна: {c} ({c_matches}) ▾")
                }
                None => "Страна ▾".to_string(),
            };
            egui::ComboBox::from_id_salt("country_facet")
                .selected_text(
                    RichText::new(country_text)
                        .color(if self.selected_country.is_some() {
                            Color32::WHITE
                        } else {
                            MUTED
                        })
                        .size(12.0)
                        .strong(),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.selected_country, None, "Все страны");
                    let mut countries: std::collections::BTreeSet<String> = self
                        .stations
                        .iter()
                        .map(|s| s.country.trim().to_uppercase())
                        .filter(|c| !c.is_empty())
                        .collect();
                    for default_c in ["US", "RU", "GB", "DE", "FR", "CA", "NL", "IT", "ES"] {
                        countries.insert(default_c.to_string());
                    }
                    for c in countries {
                        let c_count = self
                            .stations
                            .iter()
                            .filter(|s| s.country.eq_ignore_ascii_case(&c))
                            .count();
                        let name = match c.as_str() {
                            "US" => "US · США",
                            "RU" => "RU · Россия",
                            "GB" => "GB · Великобритания",
                            "DE" => "DE · Германия",
                            "FR" => "FR · Франция",
                            "CA" => "CA · Канада",
                            "NL" => "NL · Нидерланды",
                            "IT" => "IT · Италия",
                            "ES" => "ES · Испания",
                            other => other,
                        };
                        let label = if c_count > 0 {
                            format!("{name} ({c_count})")
                        } else {
                            name.to_string()
                        };
                        ui.selectable_value(&mut self.selected_country, Some(c.clone()), label);
                    }
                });

            // Bitrate facet ComboBox
            let bitrate_text = match self.selected_min_bitrate {
                Some(b) => {
                    let b_matches = self.stations.iter().filter(|s| s.bitrate >= b).count();
                    format!("≥ {b}k ({b_matches}) ▾")
                }
                None => "Битрейт ▾".to_string(),
            };
            egui::ComboBox::from_id_salt("bitrate_facet")
                .selected_text(
                    RichText::new(bitrate_text)
                        .color(if self.selected_min_bitrate.is_some() {
                            Color32::WHITE
                        } else {
                            MUTED
                        })
                        .size(12.0)
                        .strong(),
                )
                .show_ui(ui, |ui| {
                    let b128_cnt = self.stations.iter().filter(|s| s.bitrate >= 128).count();
                    let b192_cnt = self.stations.iter().filter(|s| s.bitrate >= 192).count();
                    let b320_cnt = self.stations.iter().filter(|s| s.bitrate >= 320).count();
                    ui.selectable_value(&mut self.selected_min_bitrate, None, "Любой битрейт");
                    ui.selectable_value(
                        &mut self.selected_min_bitrate,
                        Some(128),
                        format!("≥ 128 kbps ({b128_cnt})"),
                    );
                    ui.selectable_value(
                        &mut self.selected_min_bitrate,
                        Some(192),
                        format!("≥ 192 kbps (HQ) ({b192_cnt})"),
                    );
                    ui.selectable_value(
                        &mut self.selected_min_bitrate,
                        Some(320),
                        format!("≥ 320 kbps (Hi-Fi) ({b320_cnt})"),
                    );
                });

            let any_filter_active = has_facets || self.selected_genre.is_some();
            if any_filter_active {
                let reset_btn = egui::Button::new(
                    RichText::new("✕ Сбросить").size(11.0).color(MUTED),
                )
                .corner_radius(CornerRadius::same(12))
                .stroke(Stroke::new(1.0, BORDER))
                .fill(PANEL_2);
                if ui
                    .add(reset_btn)
                    .on_hover_text("Сбросить выбранные фильтры жанра, страны и битрейта")
                    .clicked()
                {
                    self.selected_genre = None;
                    self.selected_country = None;
                    self.selected_min_bitrate = None;
                    search_requested = true;
                }
            }

            if has_facets {
                ui.add_space(2.0);
                let count_info = match self.station_search_total {
                    Some(total) if total > total_stations => {
                        format!("В списке: {matching_count} из {total_stations} (каталог {total})")
                    }
                    _ => format!("В списке: {matching_count} из {total_stations}"),
                };
                ui.label(RichText::new(count_info).size(11.0).color(MUTED));
            }
        });

        search_requested
    }
}
