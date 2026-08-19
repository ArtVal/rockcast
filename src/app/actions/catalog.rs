//! Station catalog and output device refresh.

use std::time::Duration;

use crate::{
    i18n::Lang,
    output::scan_streaming,
    stations::{enrich_stations, load_catalog},
};

use super::super::{messages::UiMsg, RockCastApp};

impl RockCastApp {
    pub(in crate::app) fn bootstrap(&mut self) {
        if self.bootstrapped {
            return;
        }
        self.bootstrapped = true;
        self.refresh_stations();
        self.refresh_devices();
    }

    pub(in crate::app) fn refresh_stations(&mut self) {
        if self.loading_stations {
            return;
        }
        self.loading_stations = true;
        self.status = self.lang.t().loading_stations_status.into();
        let tx = self.ui_tx.clone();
        let lang = self.lang;
        let rockserver_enabled = self.rockserver_enabled;
        let rockserver_url = self.rockserver_url.clone();
        let rockserver_token = self.rockserver_bearer_token.clone();
        let _ = self.playback.spawn_job(move |cancel| {
            if cancel.is_cancelled() {
                return;
            }
            let (catalog, source) = load_catalog(lang);
            let _ = tx.send(UiMsg::Stations {
                list: catalog.clone(),
                source,
                finished: false,
            });
            if rockserver_enabled && !rockserver_token.trim().is_empty() {
                let locale = match lang {
                    Lang::Ru => "ru",
                    Lang::En => "en",
                };
                match crate::rockserver::search(&rockserver_url, &rockserver_token, "", locale) {
                    Ok(stations) if !stations.is_empty() => {
                        if cancel.is_cancelled() {
                            return;
                        }
                        let n = stations.len();
                        let _ = tx.send(UiMsg::Stations {
                            list: stations,
                            source: format!("RockServer · {n}"),
                            finished: true,
                        });
                        return;
                    }
                    Ok(_) => log::info!("RockServer returned empty station list, falling back"),
                    Err(e) => log::warn!("RockServer search failed: {e}; falling back"),
                }
            }
            let (merged, source) = enrich_stations(catalog, lang);
            if cancel.is_cancelled() {
                return;
            }
            let _ = tx.send(UiMsg::Stations {
                list: merged,
                source,
                finished: true,
            });
        });
    }

    pub(in crate::app) fn refresh_devices(&mut self) {
        if self.loading_devices {
            return;
        }
        self.loading_devices = true;
        self.status = self.lang.t().searching_devices.into();
        let tx = self.ui_tx.clone();
        let lang = self.lang;
        let _ = self.playback.spawn_job(move |cancel| {
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
        });
    }
}
