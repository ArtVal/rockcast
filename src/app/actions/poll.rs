//! Poll playback events and background UiMsg queue.

mod account;
mod catalog;
mod devices;
mod playback;
mod voice;

use eframe::egui;

use super::super::{RockCastApp, messages::UiMsg};

impl RockCastApp {
    pub(in crate::app) fn poll_messages(&mut self, ctx: &egui::Context) {
        if let Some(recording) = &self.voice_recording
            && !recording.load(std::sync::atomic::Ordering::Acquire)
        {
            self.voice_recording = None;
            self.status = "Распознаю команду…".into();
        }

        let relay_url = self.playback.relay_public_url();
        for title in self.observers.poll(
            self.playback.current_generation(),
            self.playing,
            self.eq_enabled,
            relay_url.as_deref(),
        ) {
            self.track_metadata = super::super::bounded_track_title(&title, &self.station_now);
            self.track = title;
        }

        while let Some(event) = self.playback.try_event() {
            if !self.playback.apply_event(&event) {
                self.finish_stale_remote_command();
                log::debug!("stale playback event ignored");
                continue;
            }
            self.handle_playback_event(event);
        }

        while let Ok(msg) = self.ui_rx.try_recv() {
            self.handle_ui_msg(ctx, msg);
        }
    }

    fn handle_ui_msg(&mut self, ctx: &egui::Context, msg: UiMsg) {
        match msg {
            UiMsg::Stations {
                list,
                source,
                request_id,
                finished,
            } => self.handle_stations_loaded(list, source, request_id, finished),
            UiMsg::StationIcon { request_key, image } => {
                self.handle_station_icon(ctx, request_key, image);
            }
            UiMsg::DeviceFound(device) => self.handle_device_found(device),
            UiMsg::DevicesFinished(status) => self.handle_devices_finished(status),
            UiMsg::RemoteChromecastDiscovery { command_id, result } => {
                self.handle_remote_chromecast_discovery(command_id, result);
            }
            UiMsg::VoiceResult(result) => self.handle_voice_result(result),
            UiMsg::PairingStarted { name, result } => self.handle_pairing_started(name, result),
            UiMsg::AccountLoaded(result) => self.handle_account_loaded(result),
            UiMsg::StationResolved { station_id, result } => {
                self.handle_station_resolved(station_id, result);
            }
            UiMsg::PersonalSyncResult { generation, result } => {
                self.handle_personal_sync_result(generation, result);
            }
            UiMsg::PairingResult { request_id, result } => {
                self.handle_pairing_result(request_id, result);
            }
        }
    }
}
