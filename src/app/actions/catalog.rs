//! Station catalog and output device refresh.

use std::time::Duration;

use crate::{
    i18n::Lang,
    output::scan_streaming,
    rockserver::RuntimeConfig,
    stations::{Station, load_catalog},
};

use super::super::{RockCastApp, messages::UiMsg};

/// Resolves one favourite known only from its profile record: checks the
/// local catalog snapshot first by ID, then queries RockServer strictly
/// by ID (`GET /api/v1/catalog/stations/{station_id}`). No search by name.
fn resolve_missing_station(
    config: &RuntimeConfig,
    station_id: &str,
    name: &str,
) -> Result<Station, String> {
    if let Some(station) = crate::stations::catalog_stations()
        .into_iter()
        .find(|s| {
            s.id == station_id
                || s.legacy_ids.iter().any(|lid| {
                    lid == station_id
                        || lid.strip_prefix("rockmobile:rockcast-")
                            .is_some_and(|hash| station_id == format!("legacy-{hash}"))
                })
        })
    {
        return Ok(station);
    }
    crate::rockserver::get_station(config, station_id)
        .map_err(|e| format!("Станция «{name}» ({station_id}) не найдена: {e}"))
}

impl RockCastApp {
    /// Starts a background lookup for a favourite whose station is not in the
    /// loaded list; the muted row's play button leads here.
    pub(in crate::app) fn begin_resolve_missing_station(&mut self, station_id: &str, name: &str) {
        if self.resolving_stations.contains(station_id) {
            return;
        }
        self.resolving_stations.insert(station_id.to_owned());
        self.status = format!("Ищу станцию «{name}»…");
        log::info!("favourite resolve started: station_id={station_id} name={name}");
        let ui_tx = self.ui_tx.clone();
        let rockserver = self.rockserver.clone();
        let request_id = station_id.to_owned();
        let name = name.to_owned();
        if self
            .background
            .spawn(move |_cancel| {
                let result = resolve_missing_station(&rockserver, &request_id, &name);
                let _ = ui_tx.send(UiMsg::StationResolved {
                    station_id: request_id,
                    result,
                });
            })
            .is_err()
        {
            self.resolving_stations.remove(station_id);
        }
    }
}

impl RockCastApp {
    pub(in crate::app) fn bootstrap(&mut self) {
        if self.bootstrapped {
            return;
        }
        self.bootstrapped = true;
        self.refresh_stations();
        self.refresh_devices();
        self.ensure_account_loaded();
    }

    pub(in crate::app) fn refresh_stations(&mut self) {
        self.search_stations(String::new());
    }

    pub(in crate::app) fn search_stations(&mut self, query: String) {
        if self.loading_stations {
            // The previous request cannot be cancelled once HTTP is in flight, but
            // its messages are tagged and ignored below.
        }
        self.station_request_id = self.station_request_id.wrapping_add(1);
        let request_id = self.station_request_id;
        self.loading_stations = true;
        self.loading_more_stations = false;
        self.loading_more_error = None;
        self.station_search_total = None;
        self.station_search_offset = 0;
        self.station_has_more = false;
        self.status = self.lang.t().loading_stations_status.into();
        let tx = self.ui_tx.clone();
        let lang = self.lang;
        let rockserver = self.rockserver.clone();
        let query = query.trim().to_owned();
        if self
            .background
            .spawn(move |cancel| {
                if cancel.is_cancelled() {
                    return;
                }
                let locale = match lang {
                    Lang::Ru => "ru",
                    Lang::En => "en",
                };
                if !query.is_empty() {
                    match crate::rockserver::search(&rockserver, &query, locale, 20, 0) {
                        Ok(batch) => {
                            if cancel.is_cancelled() {
                                return;
                            }
                            let n = batch.stations.len();
                            let total = batch.total;
                            let has_more = batch.has_more;
                            let _ = tx.send(UiMsg::Stations {
                                list: batch.stations,
                                source: if total > n {
                                    format!("RockServer · {n} / {total}")
                                } else {
                                    format!("RockServer · {n}")
                                },
                                request_id,
                                finished: true,
                                total: Some(total),
                                has_more,
                            });
                            return;
                        }
                        Err(e) => log::warn!("RockServer search failed: {e}; falling back"),
                    }
                    let (catalog, source) = load_catalog(lang);
                    let list: Vec<Station> = catalog
                        .into_iter()
                        .filter(|station| station_matches(station, &query))
                        .collect();
                    let n = list.len();
                    let _ = tx.send(UiMsg::Stations {
                        list,
                        source: format!("{source} · offline results"),
                        request_id,
                        finished: true,
                        total: Some(n),
                        has_more: false,
                    });
                    return;
                }

                let (catalog, source) = load_catalog(lang);
                if cancel.is_cancelled() {
                    return;
                }
                let n = catalog.len();
                let _ = tx.send(UiMsg::Stations {
                    list: catalog,
                    source,
                    request_id,
                    finished: true,
                    total: Some(n),
                    has_more: false,
                });
            })
            .is_err()
        {
            self.loading_stations = false;
            self.status = self.lang.t().background_busy.into();
        }
    }

    pub(in crate::app) fn load_more_stations(&mut self) {
        if self.loading_stations || self.loading_more_stations || !self.station_has_more {
            return;
        }
        let request_id = self.station_request_id;
        let offset = self.stations.len();
        self.loading_more_stations = true;
        self.loading_more_error = None;
        let tx = self.ui_tx.clone();
        let lang = self.lang;
        let rockserver = self.rockserver.clone();
        let query = self.global_station_query();
        if self
            .background
            .spawn(move |cancel| {
                if cancel.is_cancelled() {
                    return;
                }
                let locale = match lang {
                    Lang::Ru => "ru",
                    Lang::En => "en",
                };
                match crate::rockserver::search(&rockserver, &query, locale, 20, offset) {
                    Ok(batch) => {
                        if cancel.is_cancelled() {
                            return;
                        }
                        let _ = tx.send(UiMsg::MoreStationsLoaded {
                            list: batch.stations,
                            request_id,
                            offset,
                            total: batch.total,
                            has_more: batch.has_more,
                        });
                    }
                    Err(e) => {
                        if cancel.is_cancelled() {
                            return;
                        }
                        let _ = tx.send(UiMsg::MoreStationsFailed {
                            request_id,
                            error: e,
                        });
                    }
                }
            })
            .is_err()
        {
            self.loading_more_stations = false;
        }
    }

    pub(in crate::app) fn refresh_devices(&mut self) {
        if self.loading_devices {
            return;
        }
        self.loading_devices = true;
        self.status = self.lang.t().searching_devices.into();
        let tx = self.ui_tx.clone();
        let lang = self.lang;
        if self
            .background
            .spawn(move |cancel| {
                if cancel.is_cancelled() {
                    return;
                }
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    scan_streaming(Duration::from_secs(6), lang, |device| {
                        if !cancel.is_cancelled() {
                            let _ = tx.send(UiMsg::DeviceFound(device));
                        }
                    })
                }));
                match result {
                    Ok(status) => {
                        let _ = tx.send(UiMsg::DevicesFinished(status));
                    }
                    Err(_) => {
                        let _ = tx.send(UiMsg::DevicesFinished(lang.t().scan_panic.into()));
                    }
                }
            })
            .is_err()
        {
            self.loading_devices = false;
            self.status = self.lang.t().scan_panic.into();
        }
    }
}

fn station_matches(station: &crate::stations::Station, query: &str) -> bool {
    let haystack = format!(
        "{} {} {} {}",
        station.name,
        station.tags,
        station.country,
        station.aliases.join(" ")
    )
    .to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|term| haystack.contains(term))
}
