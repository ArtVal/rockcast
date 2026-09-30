//! Station catalog and output device refresh.

use std::time::Duration;

use crate::{
    i18n::Lang,
    output::scan_streaming,
    rockserver::RuntimeConfig,
    stations::{Station, load_catalog},
};

use super::super::{RockCastApp, messages::UiMsg};

fn normalize_for_match(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn rb_url_hash(url: &str) -> u64 {
    url.bytes().fold(0_u64, |hash, byte| {
        hash.wrapping_mul(109).wrapping_add(byte as u64)
    })
}

fn match_station_candidate(candidates: &[Station], name: &str, station_id: &str) -> Option<Station> {
    // 1. Direct ID match
    if let Some(s) = candidates.iter().find(|s| s.id == station_id) {
        return Some(s.clone());
    }
    // 2. Legacy radio-browser hash match
    if let Some(hash_str) = station_id.strip_prefix("radio-browser-") {
        if let Ok(expected_hash) = hash_str.parse::<u64>() {
            if let Some(s) = candidates.iter().find(|s| {
                rb_url_hash(&s.url) == expected_hash
                    || s.streams.iter().any(|st| rb_url_hash(&st.url) == expected_hash)
            }) {
                return Some(s.clone());
            }
        }
    }
    // 3. Legacy txt sha256 prefix match
    if let Some(expected_hex) = station_id.strip_prefix("legacy-") {
        if let Some(s) = candidates.iter().find(|s| {
            crate::stations::sha256_hex(s.url.as_bytes()).starts_with(expected_hex)
                || s.streams.iter().any(|st| {
                    crate::stations::sha256_hex(st.url.as_bytes()).starts_with(expected_hex)
                })
        }) {
            return Some(s.clone());
        }
    }
    // 4. Exact normalized name match
    let target_norm = normalize_for_match(name);
    if !target_norm.is_empty() {
        if let Some(s) = candidates.iter().find(|s| normalize_for_match(&s.name) == target_norm) {
            return Some(s.clone());
        }
        // Unambiguous substring match
        let matches: Vec<&Station> = candidates
            .iter()
            .filter(|s| {
                let cand_norm = normalize_for_match(&s.name);
                cand_norm.contains(&target_norm) || target_norm.contains(&cand_norm)
            })
            .collect();
        if matches.len() == 1 {
            return Some((*matches[0]).clone());
        }
    }
    None
}

/// Resolves one favourite known only from its profile record:
/// 1. Checks local catalog snapshot first by ID, legacy IDs, or name.
/// 2. Queries RockServer strictly by ID.
/// 3. Falls back to querying RockServer search by sanitized name, matching by URL hash or name.
fn resolve_missing_station(
    config: &RuntimeConfig,
    station_id: &str,
    name: &str,
) -> Result<Station, String> {
    // 1. Local catalog snapshot check
    let local = crate::stations::catalog_stations();
    if let Some(station) = match_station_candidate(&local, name, station_id) {
        return Ok(station);
    }
    if let Some(station) = local.into_iter().find(|s| {
        s.id == station_id
            || s.legacy_ids.iter().any(|lid| {
                lid == station_id
                    || lid.strip_prefix("rockmobile:rockcast-")
                        .is_some_and(|hash| station_id == format!("legacy-{hash}"))
            })
    }) {
        return Ok(station);
    }

    // 2. Direct ID lookup on RockServer
    if let Ok(station) = crate::rockserver::get_station(config, station_id) {
        return Ok(station);
    }

    // 3. Fallback: search RockServer by sanitized station name
    let clean_query: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let clean_query = clean_query.split_whitespace().collect::<Vec<_>>().join(" ");
    if !clean_query.is_empty() {
        for locale in ["ru", "en"] {
            if let Ok(batch) = crate::rockserver::search(config, &clean_query, locale, 20, 0) {
                if let Some(station) = match_station_candidate(&batch.stations, name, station_id) {
                    return Ok(station);
                }
            }
        }
    }

    Err(format!("Станция «{name}» ({station_id}) не найдена на сервере"))
}

impl RockCastApp {
    /// Starts a background lookup for a station not in the loaded list;
    /// if `auto_play` is true, status is updated and playback starts once resolved.
    pub(in crate::app) fn begin_resolve_missing_station(
        &mut self,
        station_id: &str,
        name: &str,
        auto_play: bool,
    ) {
        if self.resolving_stations.contains(station_id) {
            return;
        }
        self.resolving_stations.insert(station_id.to_owned());
        if auto_play {
            self.status = format!("Ищу станцию «{name}»…");
        }
        log::info!("station resolve started: station_id={station_id} name={name} auto_play={auto_play}");
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
                    auto_play,
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
        self.station_catalog_cursor = None;
        self.station_has_more = false;
        if !self.playing && !self.playing_op && !self.pending_voice_play {
            self.status = self.lang.t().loading_stations_status.into();
        }
        let tx = self.ui_tx.clone();
        let lang = self.lang;
        let rockserver = self.rockserver.clone();
        let query = query.trim().to_owned();
        self.active_search_query = if query.is_empty() {
            None
        } else {
            Some(query.clone())
        };
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
                let _ = tx.send(UiMsg::Stations {
                    list: catalog,
                    source,
                    request_id,
                    finished: true,
                    total: None,
                    has_more: true,
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
        let offset = self.station_search_offset;
        let cursor = self.station_catalog_cursor.clone();
        let query = self.active_search_query.clone().unwrap_or_default();
        self.loading_more_stations = true;
        self.loading_more_error = None;
        let tx = self.ui_tx.clone();
        let lang = self.lang;
        let rockserver = self.rockserver.clone();
        if self
            .background
            .spawn(move |cancel| {
                if cancel.is_cancelled() {
                    return;
                }
                if query.trim().is_empty() {
                    match crate::rockserver::list_catalog(&rockserver, cursor.as_deref(), 20) {
                        Ok(batch) => {
                            if cancel.is_cancelled() {
                                return;
                            }
                            let has_more = batch.next_cursor.is_some() && !batch.stations.is_empty();
                            let next_cursor = batch.next_cursor;
                            let _ = tx.send(UiMsg::MoreStationsLoaded {
                                list: batch.stations,
                                request_id,
                                offset: 0,
                                total: None,
                                has_more,
                                next_cursor,
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
                } else {
                    let locale = match lang {
                        Lang::Ru => "ru",
                        Lang::En => "en",
                    };
                    match crate::rockserver::search(&rockserver, &query, locale, 20, offset) {
                        Ok(batch) => {
                            if cancel.is_cancelled() {
                                return;
                            }
                            let n = batch.stations.len();
                            let next_offset = offset + n;
                            let _ = tx.send(UiMsg::MoreStationsLoaded {
                                list: batch.stations,
                                request_id,
                                offset: next_offset,
                                total: Some(batch.total),
                                has_more: batch.has_more,
                                next_cursor: None,
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
    fn match_station_candidate_matches_by_id() {
        let candidates = vec![test_station("rb-123", "Station Name", "https://stream/1")];
        let matched = match_station_candidate(&candidates, "Different Name", "rb-123");
        assert_eq!(matched.unwrap().id, "rb-123");
    }

    #[test]
    fn match_station_candidate_matches_by_radio_browser_hash() {
        let url = "https://streaming.exclusive.radio/er/blacksabbath/icecast.audio";
        let expected_hash = rb_url_hash(url);
        let station_id = format!("radio-browser-{expected_hash}");

        let candidates = vec![test_station("rb-black-sabbath", "Exclusive Radio – Black Sabbath", url)];
        let matched = match_station_candidate(&candidates, "Exclusive Radio – Black Sabbath", &station_id);
        assert_eq!(matched.unwrap().id, "rb-black-sabbath");
    }

    #[test]
    fn match_station_candidate_matches_by_legacy_txt_sha() {
        let url = "https://listen.181fm.com/181-hairband_128k.mp3";
        let sha_prefix = &crate::stations::sha256_hex(url.as_bytes())[..16];
        let station_id = format!("legacy-{sha_prefix}");

        let candidates = vec![test_station("181-fm-hair-band", "181.FM — Hair Band", url)];
        let matched = match_station_candidate(&candidates, "181.FM — Hair Band", &station_id);
        assert_eq!(matched.unwrap().id, "181-fm-hair-band");
    }

    #[test]
    fn match_station_candidate_matches_by_normalized_name_with_dashes() {
        let candidates = vec![test_station("rb-target", "Exclusive Radio — Black Sabbath", "https://new-stream/url")];
        let matched = match_station_candidate(&candidates, "Exclusive Radio – Black Sabbath", "radio-browser-99999");
        assert_eq!(matched.unwrap().id, "rb-target");
    }
}
