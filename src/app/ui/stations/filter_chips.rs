//! Filter chips for All, Favourites, History, and Genre selection.

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, Ui};

use crate::app::{
    RockCastApp, StationFilterMode,
    theme::{ACCENT, BORDER, FG, GOLD_STAR, MUTED, PANEL},
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

            // Filter chips: All, Favourites, History, Genres
            let all_selected = self.filter_mode == StationFilterMode::All
                && self.selected_genre.is_none();
            let all_btn = egui::Button::new(
                RichText::new(format!("Все ({})", self.stations.len()))
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
                self.filter_mode = StationFilterMode::All;
                self.selected_genre = None;
                search_requested = true;
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
                    // Genre chips drive a server search via global_station_query()
                    // instead of filtering the local catalog in place.
                    search_requested = true;
                }
            }
        });

        search_requested
    }
}
