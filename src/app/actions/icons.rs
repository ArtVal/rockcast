//! Station-icon fetch scheduling. This file only submits bounded jobs; it
//! never performs network work on the egui thread.

use crate::{station_icons, stations::Station};

use super::super::{RockCastApp, messages::UiMsg};

impl RockCastApp {
    pub(in crate::app) fn queue_station_icons(&mut self, stations: &[Station]) {
        let root = station_icons::cache_dir();
        for station in stations {
            let Some(source) = station_icons::source_url(station) else {
                continue;
            };
            let request_key = station_icons::request_key(station, &source);
            if !self.station_icon_requests.insert(request_key.clone()) {
                continue;
            }
            let tx = self.ui_tx.clone();
            let station = station.clone();
            let request_key_for_job = request_key.clone();
            let root = root.clone();
            let result = self.playback.spawn_job(move |cancel| {
                if cancel.is_cancelled() {
                    return;
                }
                let image = match station_icons::load_or_fetch(&station, &root) {
                    Ok(image) => image,
                    Err(error) => {
                        // The request identity remains recorded, so a bad
                        // host/image cannot cause a download on every redraw.
                        log::debug!(
                            "station icon unavailable: station_id={} error={error}",
                            station.id
                        );
                        None
                    }
                };
                if !cancel.is_cancelled() {
                    let _ = tx.send(UiMsg::StationIcon {
                        request_key: request_key_for_job,
                        image,
                    });
                }
            });
            if result.is_ok() {
                self.station_icons_pending += 1;
            } else {
                log::debug!("station icon job could not be scheduled");
            }
        }
    }
}
