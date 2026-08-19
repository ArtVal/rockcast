//! Poll playback events and background UiMsg queue.

use crate::{
    i18n,
    playback::PlaybackEvent,
};

use super::super::{messages::{UiMsg, same_output_device}, RockCastApp};

impl RockCastApp {
    pub(in crate::app) fn poll_messages(&mut self) {
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
                    self.playing_url = Some(url);
                    self.track = self.lang.t().track_meta_hint.into();
                    if !local && !self.cast_relay && self.eq_enabled {
                        if let Some(tap_url) = tap_url {
                            self.schedule_stream_tap(generation, tap_url);
                        }
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
                    self.source = source;
                    self.restore_station_selection();
                    self.loading_stations = !finished;
                    self.status = i18n::fmt1(self.lang.t().stations_count, self.stations.len());
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
                            let stations = result.stations;
                            log::info!("voice candidates received: count={}", stations.len());
                            let first = stations[0].clone();
                            self.voice_fallback = stations.iter().skip(1).cloned().collect();
                            self.station_now = first.name.clone();
                            self.stations = stations;
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
                            crate::voice_prompts::play(
                                crate::voice_prompts::Prompt::NotFound,
                                self.lang,
                            );
                            self.status = format!("Голосовое управление: {error}");
                        }
                    }
                }
            }
        }
    }
}
