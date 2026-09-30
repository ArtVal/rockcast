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
        mut list: Vec<Station>,
        source: String,
        request_id: u64,
        finished: bool,
        mut total: Option<usize>,
        has_more: bool,
    ) {
        if request_id != self.station_request_id {
            return;
        }
        let playing_id = self.playback_station_id.as_deref();
        let playing_url = self.settings.station_url.as_deref();
        preserve_playing_station(
            playing_id,
            playing_url,
            &self.stations,
            &mut list,
            &mut total,
        );
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
        if !self.playing && !self.playing_op && !self.pending_voice_play {
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

pub(crate) fn preserve_playing_station(
    playing_id: Option<&str>,
    playing_url: Option<&str>,
    current_stations: &[Station],
    list: &mut Vec<Station>,
    total: &mut Option<usize>,
) {
    if playing_id.is_none() && playing_url.is_none() {
        return;
    }
    let already_in_list = list.iter().any(|s| {
        (playing_id.is_some() && Some(s.id.as_str()) == playing_id)
            || (playing_url.is_some() && !s.url.is_empty() && Some(s.url.as_str()) == playing_url)
    });
    if !already_in_list {
        if let Some(playing_st) = current_stations.iter().find(|s| {
            (playing_id.is_some() && Some(s.id.as_str()) == playing_id)
                || (playing_url.is_some() && !s.url.is_empty() && Some(s.url.as_str()) == playing_url)
        }).cloned() {
            list.insert(0, playing_st);
            if let Some(tot) = total.as_mut() {
                *tot += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_station(id: &str, name: &str, url: &str) -> Station {
        Station::from_primary(
            id.to_string(),
            name.to_string(),
            url.to_string(),
            "rock".to_string(),
            "RU".to_string(),
            128,
            "mp3".to_string(),
        )
    }

    #[test]
    fn preserve_playing_station_prepends_missing_playing_station() {
        let playing = test_station("st-1", "Playing Station", "http://stream.test/1");
        let other = test_station("st-2", "Other Station", "http://stream.test/2");

        let current = vec![playing.clone()];
        let mut list = vec![other.clone()];
        let mut total = Some(10);

        preserve_playing_station(
            Some("st-1"),
            Some("http://stream.test/1"),
            &current,
            &mut list,
            &mut total,
        );

        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "st-1");
        assert_eq!(list[1].id, "st-2");
        assert_eq!(total, Some(11));
    }

    #[test]
    fn preserve_playing_station_does_not_duplicate_existing_station() {
        let playing = test_station("st-1", "Playing Station", "http://stream.test/1");

        let current = vec![playing.clone()];
        let mut list = vec![playing.clone()];
        let mut total = Some(10);

        preserve_playing_station(
            Some("st-1"),
            Some("http://stream.test/1"),
            &current,
            &mut list,
            &mut total,
        );

        assert_eq!(list.len(), 1);
        assert_eq!(total, Some(10));
    }

    #[test]
    fn preserve_playing_station_noop_when_not_playing() {
        let other = test_station("st-2", "Other Station", "http://stream.test/2");
        let current = vec![];
        let mut list = vec![other.clone()];
        let mut total = Some(10);

        preserve_playing_station(None, None, &current, &mut list, &mut total);

        assert_eq!(list.len(), 1);
        assert_eq!(total, Some(10));
    }
}

