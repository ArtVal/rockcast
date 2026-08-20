//! Persisted settings and UI preference toggles.

use std::time::{Duration, Instant};

use crate::i18n::{self, Lang};

use super::super::RockCastApp;

impl RockCastApp {
    pub(in crate::app) fn restore_station_selection(&mut self) {
        if let Some(url) = self.settings.station_url.as_ref()
            && let Some(i) = self.stations.iter().position(|s| &s.url == url)
        {
            self.selected_station = Some(i);
            return;
        }
        if self.selected_station.is_none() && !self.stations.is_empty() {
            self.selected_station = Some(0);
        }
    }

    pub(in crate::app) fn restore_device_selection(&mut self) {
        if self.devices.is_empty() {
            self.selected_device = None;
            return;
        }
        if let Some(id) = self.settings.device_id.as_ref()
            && let Some(i) = self.devices.iter().position(|d| d.id() == id)
        {
            self.selected_device = Some(i);
            return;
        }
        // A fresh profile must never start audio on a network receiver
        // unexpectedly. Local devices are ordered with the Windows default first.
        self.selected_device = self
            .devices
            .iter()
            .position(|device| device.is_local())
            .or(Some(0));
    }

    pub(in crate::app) fn mark_settings_dirty(&mut self) {
        self.settings.volume = self.volume;
        self.settings.eq_enabled = self.eq_enabled;
        self.settings.cast_relay = self.cast_relay;
        self.settings.language = self.lang;
        self.settings.rockserver_enabled = self.rockserver_enabled;
        self.settings.rockserver_url = self.rockserver_url.trim().to_owned();
        self.settings.rockserver_bearer_token = self.rockserver_bearer_token.trim().to_owned();
        self.settings.rockserver_voice_mode = self.rockserver_voice_mode;
        if let Some(url) = self
            .selected_station
            .and_then(|i| self.stations.get(i).map(|s| s.url.clone()))
        {
            self.settings.station_url = Some(url);
        }
        // Don't clear saved device while the list is still empty / loading.
        if let Some(id) = self
            .selected_device
            .and_then(|i| self.devices.get(i).map(|d| d.id().to_string()))
        {
            self.settings.device_id = Some(id);
        }
        self.settings_dirty = true;
    }

    pub(in crate::app) fn persist_settings_if_needed(&mut self, force: bool) {
        if !self.settings_dirty {
            return;
        }
        if !force && self.last_settings_save.elapsed() < Duration::from_millis(400) {
            return;
        }
        self.settings_dirty = false;
        self.last_settings_save = Instant::now();
        if let Err(e) = self.settings.save() {
            log::warn!("failed to persist settings: {e}");
        }
    }

    pub(in crate::app) fn apply_volume_if_needed(&mut self) {
        // Volume goes out via vol_tx as soon as the slider moves.
        self.persist_settings_if_needed(false);
    }

    pub(in crate::app) fn set_eq_enabled(&mut self, enabled: bool) {
        if self.eq_enabled == enabled {
            return;
        }
        self.eq_enabled = enabled;
        self.eq_repaint_next = Instant::now();
        self.mark_settings_dirty();
        self.persist_settings_if_needed(true);
        self.sync_spectrum();
    }

    pub(in crate::app) fn set_cast_relay(&mut self, enabled: bool) {
        if self.cast_relay == enabled {
            return;
        }
        self.cast_relay = enabled;
        log::info!("cast_relay -> {enabled}");
        self.mark_settings_dirty();
        self.persist_settings_if_needed(true);

        let cast_active = (self.playing || self.playing_op) && !self.playing_local;
        if cast_active {
            log::info!("cast_relay changed during Cast playback — restarting with relay={enabled}");
            self.status = if enabled {
                self.lang.t().cast_relay_restart_on.into()
            } else {
                self.lang.t().cast_relay_restart_off.into()
            };
            self.play();
        }
    }

    pub(in crate::app) fn set_language(&mut self, ctx: &egui::Context, lang: Lang) {
        self.lang = lang;
        self.mark_settings_dirty();
        self.persist_settings_if_needed(true);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(
            lang.t().window_title.to_string(),
        ));
        if !self.playing {
            self.track = lang.t().track_hint.into();
        } else {
            self.track = lang.t().track_meta_hint.into();
        }
        if !self.loading_devices {
            self.refresh_devices();
        }
        // Refresh the catalog source label without a full RB request.
        let n = self.stations.len();
        if n > 0 {
            let localish = self.source.contains("локаль")
                || self.source.contains("local catalog")
                || !self.source.contains("Radio Browser");
            self.source = if localish {
                i18n::fmt1(lang.t().local_catalog, n)
            } else {
                i18n::fmt1(lang.t().catalog_plus_rb, n)
            };
        }
    }
}
