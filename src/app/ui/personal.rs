//! Minimal local favourites/history controls; no network or playback ownership.

use eframe::egui::{self, Context, RichText};

use super::super::{
    RockCastApp,
    theme::{ACCENT, MUTED},
};

impl RockCastApp {
    fn play_personal_station(&mut self, station_id: &str) {
        let Some(index) = crate::personal_data::station_index_by_id(&self.stations, station_id)
        else {
            self.status = format!("Station unavailable: {station_id}");
            return;
        };
        self.selected_station = Some(index);
        self.scroll_to_station = Some(index);
        self.mark_settings_dirty();
        self.play();
    }

    pub(in crate::app) fn toggle_selected_favourite(&mut self) {
        let Some(station) = self
            .selected_station
            .and_then(|index| self.stations.get(index))
            .cloned()
        else {
            return;
        };
        let Some(store) = self.personal_data.as_mut() else {
            return;
        };
        match store.toggle_favourite(&station) {
            Ok(true) => self.status = format!("Added to favourites: {}", station.name),
            Ok(false) => self.status = format!("Removed from favourites: {}", station.name),
            Err(error) => self.status = format!("Favourites unavailable: {error}"),
        }
    }

    pub(in crate::app) fn draw_personal_windows(&mut self, ctx: &Context) {
        if self.favourites_open {
            self.draw_favourites_window(ctx);
        }
        if self.history_open {
            self.draw_history_window(ctx);
        }
    }

    fn draw_favourites_window(&mut self, ctx: &Context) {
        let mut open = self.favourites_open;
        let mut selected_id = None;
        egui::Window::new("Local favourites")
            .open(&mut open)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(RichText::new("Stored only on this device").color(MUTED));
                ui.separator();
                let favourites = self
                    .personal_data
                    .as_ref()
                    .map(|store| store.favourites().to_vec())
                    .unwrap_or_default();
                let unavailable = self
                    .personal_data
                    .as_ref()
                    .map(|store| {
                        store
                            .profile()
                            .unresolved_references
                            .iter()
                            .filter(|entry| entry.source_kind == "favourite")
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if favourites.is_empty() && unavailable.is_empty() {
                    ui.label(RichText::new("No favourite stations yet.").color(MUTED));
                }
                for favourite in favourites {
                    let name = favourite
                        .metadata
                        .last_known_name
                        .unwrap_or_else(|| favourite.station_id.clone());
                    if ui
                        .button(RichText::new(name).color(ACCENT))
                        .on_hover_text("Select and play")
                        .clicked()
                    {
                        selected_id = Some(favourite.station_id);
                    }
                }
                for entry in unavailable {
                    unavailable_station(ui, entry.last_known_name, &entry.original_station_id);
                }
            });
        self.favourites_open = open;
        if let Some(station_id) = selected_id {
            self.play_personal_station(&station_id);
        }
    }

    fn draw_history_window(&mut self, ctx: &Context) {
        let mut open = self.history_open;
        let mut selected_id = None;
        egui::Window::new("Local playback history")
            .open(&mut open)
            .resizable(true)
            .show(ctx, |ui| {
                let history = self
                    .personal_data
                    .as_ref()
                    .map(|store| store.history().to_vec())
                    .unwrap_or_default();
                let unavailable = self
                    .personal_data
                    .as_ref()
                    .map(|store| {
                        store
                            .profile()
                            .unresolved_references
                            .iter()
                            .filter(|entry| entry.source_kind == "history")
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Stored only on this device").color(MUTED));
                    if ui.button("Clear history").clicked()
                        && let Some(store) = self.personal_data.as_mut()
                    {
                        match store.clear_history() {
                            Ok(()) => self.status = "Playback history cleared".into(),
                            Err(error) => self.status = format!("History unavailable: {error}"),
                        }
                    }
                });
                ui.separator();
                if history.is_empty() && unavailable.is_empty() {
                    ui.label(RichText::new("No local playback history yet.").color(MUTED));
                }
                for entry in history {
                    let name = entry
                        .metadata
                        .last_known_name
                        .unwrap_or_else(|| entry.station_id.clone());
                    if ui
                        .button(RichText::new(name).color(ACCENT))
                        .on_hover_text("Select and play")
                        .clicked()
                    {
                        selected_id = Some(entry.station_id.clone());
                    }
                    ui.label(
                        RichText::new(format!("{} · {}", entry.station_id, entry.last_played_at))
                            .color(MUTED)
                            .small(),
                    );
                    ui.add_space(4.0);
                }
                for entry in unavailable {
                    unavailable_station(ui, entry.last_known_name, &entry.original_station_id);
                }
            });
        self.history_open = open;
        if let Some(station_id) = selected_id {
            self.play_personal_station(&station_id);
        }
    }
}

fn unavailable_station(ui: &mut egui::Ui, name: Option<String>, station_id: &str) {
    ui.label(
        RichText::new(name.unwrap_or_else(|| station_id.to_owned()))
            .color(MUTED)
            .strikethrough(),
    );
    ui.label(
        RichText::new(format!("Station unavailable · {station_id}"))
            .color(MUTED)
            .small(),
    );
    ui.add_space(4.0);
}
