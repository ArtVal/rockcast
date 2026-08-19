//! Play, stop, shutdown, and observer wiring.

use super::super::RockCastApp;

impl RockCastApp {
    pub(in crate::app) fn queue_volume(&self) {
        let is_local = self
            .selected_device
            .and_then(|i| self.devices.get(i))
            .is_some_and(|d| d.is_local())
            || self.playing_local;
        self.playback.set_volume(is_local, self.volume);
    }

    pub(in crate::app) fn shutdown_playback(&mut self) {
        if self.shutting_down {
            return;
        }
        let generation = self.playback.current_generation() + 1;
        log::info!(
            "shutdown_playback: bump generation→{generation} playing={} local={}",
            self.playing,
            self.playing_local
        );
        self.shutting_down = true;
        self.observers.stop();
        self.playing = false;
        self.playing_local = false;
        self.playing_url = None;
        self.mark_settings_dirty();
        self.persist_settings_if_needed(true);
        // Stop local first (non-blocking). Cast STOP is best-effort with a short wait
        // so a hung Cast handshake cannot freeze window close.
        self.playback.shutdown();
        log::info!("shutdown_playback: finished");
    }

    pub(in crate::app) fn play(&mut self) {
        if !self.can_start_play() {
            log::debug!(
                "play blocked: loading_devices={} devices={} selected_device={:?} selected_station={:?}",
                self.loading_devices,
                self.devices.len(),
                self.selected_device,
                self.selected_station
            );
            return;
        }
        let Some(station) = self
            .selected_station
            .and_then(|index| self.stations.get(index))
            .cloned()
        else {
            self.status = self.lang.t().pick_station.into();
            return;
        };
        let Some(device) = self
            .selected_device
            .and_then(|index| self.devices.get(index))
            .cloned()
        else {
            self.status = self.lang.t().pick_device.into();
            return;
        };
        let local = device.is_local();
        self.observers.stop();
        self.playing_url = None;
        self.playing_op = true;
        self.playing = false;
        self.playing_local = local;
        self.status = format!("Play: {} -> {}", station.name, device.name());
        self.station_now = station.name.clone();
        self.track = self.lang.t().connecting.into();
        self.mark_settings_dirty();
        self.persist_settings_if_needed(true);
        self.playback.play(
            station,
            device,
            self.volume,
            self.cast_relay && !local,
            self.eq_enabled,
        );
    }

    pub(in crate::app) fn stop(&mut self) {
        if self.shutting_down {
            return;
        }
        self.playing_op = true;
        self.status = "Stop…".into();
        self.pending_voice_play = false;
        self.voice_fallback.clear();
        self.observers.stop();
        self.playing = false;
        self.playing_local = false;
        self.playing_url = None;
        self.track = self.lang.t().stopped.into();
        self.playback.stop();
    }

    pub(in crate::app) fn schedule_stream_tap(&mut self, generation: u64, tap_url: String) {
        self.observers.schedule(
            generation,
            tap_url,
            self.playback.relay_active(),
        );
    }

    pub(in crate::app) fn sync_spectrum(&mut self) {
        if self.playback.relay_active() {
            self.observers.stop();
            return;
        }
        let tap = self.playing_url.clone();
        let relay_url = self.playback.relay_public_url();
        self.observers.sync(
            self.playing,
            self.playing_local,
            self.eq_enabled,
            tap,
            relay_url.as_deref(),
        );
    }
}
