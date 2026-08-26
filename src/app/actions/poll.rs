//! Poll playback events and background UiMsg queue.

use crate::{i18n, playback::PlaybackEvent};
use eframe::egui;

use super::super::{
    RockCastApp,
    messages::{UiMsg, same_output_device},
};

fn is_station_unavailable_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "404",
        "station unavailable",
        "failed to open audio stream",
        "upstream",
        "timed out",
        "timeout",
        "eof",
        "stalled in idle",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

impl RockCastApp {
    pub(in crate::app) fn poll_messages(&mut self, ctx: &egui::Context) {
        let relay_url = self.playback.relay_public_url();
        for title in self.observers.poll(
            self.playback.current_generation(),
            self.playing,
            self.eq_enabled,
            relay_url.as_deref(),
        ) {
            self.track = title;
        }
        while let Some(event) = self.playback.try_event() {
            if !self.playback.apply_event(&event) {
                log::debug!("stale playback event ignored");
                continue;
            }
            match event {
                PlaybackEvent::Status { text, .. } => self.status = text,
                PlaybackEvent::Title { title, .. } => self.track = title,
                PlaybackEvent::PlayOk {
                    url,
                    tap_url,
                    generation,
                    local,
                } => {
                    self.playing_op = false;
                    self.playing = true;
                    self.playing_local = local;
                    self.playing_url = Some(url.clone());
                    if let Some(station) = self
                        .stations
                        .iter()
                        .find(|station| station.url == url)
                        .cloned()
                    {
                        self.last_played_station = Some(station.clone());
                        if let Some(store) = self.personal_data.as_mut()
                            && let Err(error) = store.record_play(&station)
                        {
                            log::warn!("failed to record playback history: {error}");
                        }
                    }
                    self.track = self.lang.t().track_meta_hint.into();
                    if !local
                        && !self.playback.relay_active()
                        && self.eq_enabled
                        && let Some(tap_url) = tap_url
                    {
                        self.schedule_stream_tap(generation, tap_url);
                    }
                }
                PlaybackEvent::StopOk { .. } => {
                    self.playing_op = false;
                    self.playing = false;
                    self.playing_local = false;
                    self.playing_url = None;
                    self.observers.stop();
                    self.track = self.lang.t().stopped.into();
                    self.status = self.lang.t().stopped.into();
                }
                PlaybackEvent::Error { message, .. } => {
                    if is_station_unavailable_error(&message) {
                        crate::voice_prompts::play(
                            crate::voice_prompts::Prompt::StationUnavailable,
                            self.lang,
                        );
                    }
                    self.playing_op = false;
                    self.playing = false;
                    self.playing_local = false;
                    self.playing_url = None;
                    self.observers.stop();
                    let failed_url = self
                        .selected_station
                        .and_then(|index| self.stations.get(index))
                        .map(|station| station.url.clone());
                    if let Some(failed_url) = failed_url {
                        self.stations.retain(|station| station.url != failed_url);
                        log::warn!("removing unavailable station: {failed_url}: {message}");
                    }
                    self.selected_station = None;
                    self.station_now = "—".into();
                    self.track = self.lang.t().track_hint.into();
                    if let Some(next) = self.voice_fallback.pop_front() {
                        log::info!(
                            "voice fallback: trying next station name={:?} url={} remaining={}",
                            next.name,
                            next.url,
                            self.voice_fallback.len()
                        );
                        self.status =
                            format!("Станция недоступна; пробую следующую: {}", next.name);
                        self.stations.retain(|station| station.url != next.url);
                        self.stations.insert(0, next);
                        self.selected_station = Some(0);
                        self.scroll_to_station = Some(0);
                        self.play();
                    } else {
                        self.status = message;
                    }
                }
            }
        }
        while let Ok(msg) = self.ui_rx.try_recv() {
            match msg {
                UiMsg::Stations {
                    list,
                    source,
                    finished,
                } => {
                    self.stations = list;
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
                    }
                    self.source = source;
                    self.restore_station_selection();
                    self.loading_stations = !finished;
                    self.status = i18n::fmt1(self.lang.t().stations_count, self.stations.len());
                }
                UiMsg::StationIcon { request_key, image } => {
                    self.station_icons_pending = self.station_icons_pending.saturating_sub(1);
                    if let Some(image) = image
                        && self.station_icon_requests.contains(&request_key)
                    {
                        let color_image = egui::ColorImage::from_rgba_unmultiplied(
                            [image.width, image.height],
                            &image.rgba,
                        );
                        let texture = ctx.load_texture(
                            format!("station-icon-{request_key}"),
                            color_image,
                            egui::TextureOptions::LINEAR,
                        );
                        self.station_icons.insert(request_key, texture);
                    }
                }
                UiMsg::DeviceFound(device) => {
                    let selected_id = self
                        .selected_device
                        .and_then(|index| self.devices.get(index))
                        .map(|device| device.id().to_owned());
                    let kind = if device.is_local() { "local" } else { "cast" };
                    log::info!(
                        "device found incrementally: kind={kind} id={} name='{}'",
                        device.id(),
                        device.name()
                    );
                    if let Some(index) = self
                        .devices
                        .iter()
                        .position(|existing| same_output_device(existing, &device))
                    {
                        self.devices[index] = device;
                    } else {
                        self.devices.push(device);
                    }
                    self.devices.sort_by_key(|device| !device.is_local());
                    self.selected_device = selected_id
                        .as_deref()
                        .and_then(|id| self.devices.iter().position(|device| device.id() == id));
                    if self.selected_device.is_none() {
                        self.restore_device_selection();
                    }
                    self.status = format!(
                        "{} ({})",
                        self.lang.t().searching_devices,
                        self.devices.len()
                    );
                    if self.pending_voice_play && self.can_start_play() {
                        self.pending_voice_play = false;
                        log::info!("voice playback resumed after first audio device");
                        self.play();
                    }
                }
                UiMsg::DevicesFinished(status) => {
                    log::info!(
                        "device scan finished: count={} status={status}",
                        self.devices.len()
                    );
                    self.restore_device_selection();
                    self.loading_devices = false;
                    let local_n = self.devices.iter().filter(|d| d.is_local()).count();
                    let cast_n = self.devices.len().saturating_sub(local_n);
                    let selected = self
                        .selected_device
                        .and_then(|i| self.devices.get(i))
                        .map(|d| d.label(self.lang))
                        .unwrap_or_else(|| self.lang.t().device_none.into());
                    self.status = if cast_n == 0 {
                        i18n::fmt1(self.lang.t().cast_none, local_n)
                    } else {
                        i18n::fmt3(self.lang.t().cast_found, local_n, cast_n, selected)
                    };
                    if status.contains("panic") || status.contains("Ошибка") {
                        self.status = status;
                    }
                    if let Some(i) = self.selected_device {
                        log::info!(
                            "device selected after scan: idx={i} id={}",
                            self.devices.get(i).map(|d| d.id()).unwrap_or("?")
                        );
                    }
                }
                UiMsg::VoiceResult(result) => {
                    self.voice_busy = false;
                    self.voice_recording = None;
                    match result {
                        Ok(result) => {
                            if let Some(control) = result.control {
                                self.apply_voice_control(control);
                                continue;
                            }
                            let stations = result.stations;
                            log::info!("voice candidates received: count={}", stations.len());
                            let first = stations[0].clone();
                            self.voice_fallback = stations.iter().skip(1).cloned().collect();
                            self.station_now = first.name.clone();
                            self.stations = stations;
                            self.queue_station_icons(&self.stations.clone());
                            self.source = format!("RockServer · голос · {}", self.stations.len());
                            self.selected_station = Some(0);
                            self.scroll_to_station = Some(0);
                            log::info!(
                                "voice selected first station: name={:?} url={} fallbacks={}",
                                self.stations[0].name,
                                self.stations[0].url,
                                self.voice_fallback.len()
                            );
                            if result.auto_play {
                                crate::voice_prompts::play(
                                    crate::voice_prompts::Prompt::TurningOn,
                                    self.lang,
                                );
                                self.status =
                                    "Голосовая команда распознана; запускаю станцию".into();
                                if self.can_start_play() {
                                    self.play();
                                } else {
                                    self.pending_voice_play = true;
                                    self.status =
                                        "Команда распознана; ожидаю аудиоустройство…".into();
                                }
                            } else {
                                self.pending_voice_play = false;
                                self.voice_fallback.clear();
                                self.status = format!(
                                    "Найдено станций: {}. Список отсортирован по похожести.",
                                    self.stations.len()
                                );
                            }
                        }
                        Err(error) => {
                            let prompt = match &error {
                                crate::voice::VoiceError::ServerUnavailable => {
                                    Some(crate::voice_prompts::Prompt::ServerUnavailable)
                                }
                                crate::voice::VoiceError::TokenMissing => {
                                    Some(crate::voice_prompts::Prompt::TokenMissing)
                                }
                                crate::voice::VoiceError::TokenInvalid => {
                                    Some(crate::voice_prompts::Prompt::TokenInvalid)
                                }
                                crate::voice::VoiceError::NotFound => {
                                    Some(crate::voice_prompts::Prompt::NotFound)
                                }
                                crate::voice::VoiceError::Message(_) => None,
                            };
                            if let Some(prompt) = prompt {
                                crate::voice_prompts::play(prompt, self.lang);
                            }
                            self.status = format!("Голосовое управление: {error}");
                        }
                    }
                }
            }
        }
    }
}
