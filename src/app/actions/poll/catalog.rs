use std::collections::HashSet;
use eframe::egui;

use crate::{
    app::RockCastApp,
    i18n,
    station_icons::StationIconImage,
    stations::Station,
};

impl RockCastApp {
    pub(super) fn handle_stations_loaded(
        &mut self,
        list: Vec<Station>,
        source: String,
        request_id: u64,
        finished: bool,
        total: Option<usize>,
        has_more: bool,
    ) {
        if request_id != self.station_request_id {
            return;
        }
        self.stations = list;
        self.station_search_total = total;
        self.station_search_offset = self.stations.len();
        self.station_catalog_cursor = None;
        self.station_has_more = has_more;
        self.loading_more_stations = false;
        self.loading_more_error = None;
        self.queue_station_icons(&self.stations.clone());
        if self.personal_data.is_none() {
            let resolver = crate::stations::catalog_resolver();
            match crate::personal_data::PersonalDataStore::open(
                crate::personal_data::PersonalDataStore::default_path(),
                resolver,
            ) {
                Ok(store) => self.personal_data = Some(store),
                Err(error) => log::warn!("personal data disabled: {error}"),
            }
            self.init_personal_sync_state();
        }
        self.source = source;
        self.restore_station_selection();
        self.loading_stations = !finished;
        self.status = match self.station_search_total {
            Some(tot) if tot > self.stations.len() => {
                format!(
                    "{}: {} из {tot}",
                    self.lang.t().stations_count,
                    self.stations.len()
                )
            }
            _ => i18n::fmt1(self.lang.t().stations_count, self.stations.len()),
        };
    }

    pub(super) fn handle_more_stations_loaded(
        &mut self,
        list: Vec<Station>,
        request_id: u64,
        next_offset: usize,
        total: Option<usize>,
        has_more: bool,
        next_cursor: Option<String>,
    ) {
        if request_id != self.station_request_id {
            return;
        }
        self.loading_more_stations = false;
        self.loading_more_error = None;
        if let Some(tot) = total {
            self.station_search_total = Some(tot);
        }
        self.station_has_more = has_more;
        self.station_catalog_cursor = next_cursor;
        self.station_search_offset = next_offset;

        if !list.is_empty() {
            let mut existing_ids: HashSet<String> =
                self.stations.iter().map(|s| s.id.clone()).collect();
            let new_stations: Vec<Station> = list
                .into_iter()
                .filter(|s| existing_ids.insert(s.id.clone()))
                .collect();
            if !new_stations.is_empty() {
                self.queue_station_icons(&new_stations);
                self.stations.extend(new_stations);
            }
        }
        let loaded = self.stations.len();
        match self.station_search_total {
            Some(tot) => {
                self.source = format!("RockServer · {loaded} / {tot}");
                self.status = format!("{}: {loaded} из {tot}", self.lang.t().stations_count);
            }
            None => {
                self.source = format!("RockServer · {loaded}");
                self.status = i18n::fmt1(self.lang.t().stations_count, loaded);
            }
        }
    }

    pub(super) fn handle_more_stations_failed(&mut self, request_id: u64, error: String) {
        if request_id != self.station_request_id {
            return;
        }
        self.loading_more_stations = false;
        self.loading_more_error = Some(error);
    }

    pub(super) fn handle_station_icon(
        &mut self,
        ctx: &egui::Context,
        request_key: String,
        image: Option<StationIconImage>,
    ) {
        self.station_icons_pending = self.station_icons_pending.saturating_sub(1);
        if let Some(image) = image
            && self.station_icon_requests.contains(&request_key)
        {
            let color_image =
                egui::ColorImage::from_rgba_unmultiplied([image.width, image.height], &image.rgba);
            let texture = ctx.load_texture(
                format!("station-icon-{request_key}"),
                color_image,
                egui::TextureOptions::LINEAR,
            );
            self.station_icons.insert(request_key, texture);
        }
    }

    pub(super) fn handle_station_resolved(
        &mut self,
        station_id: String,
        result: Result<Station, String>,
    ) {
        self.resolving_stations.remove(&station_id);
        match result {
            Ok(station) => {
                log::info!(
                    "favourite resolved: station_id={station_id} -> {} name='{}'",
                    station.id,
                    station.name
                );
                // A rehosted stream resolves under a new id: move
                // the favourite to the found station so the row
                // stops being grey and sync spreads the fix.
                if station.id != station_id && self.is_station_favourite(&station_id) {
                    let name = station.name.clone();
                    let station_clone = station.clone();
                    if let Some(store) = self.personal_data.as_mut() {
                        let _ = store.remove_favourite(&station_id);
                        let _ = store.toggle_favourite(&station_clone);
                    }
                    self.status = format!("Станция «{name}» переехала — избранное обновлено");
                    self.schedule_personal_sync();
                }
                let index = match self.stations.iter().position(|s| s.id == station.id) {
                    Some(index) => index,
                    None => {
                        self.stations.insert(0, station);
                        0
                    }
                };
                self.selected_station = Some(index);
                self.scroll_to_station = Some(index);
                self.mark_settings_dirty();
                self.play();
            }
            Err(message) => {
                log::info!("favourite resolve failed: station_id={station_id}: {message}");
                self.status = message;
            }
        }
    }
}
