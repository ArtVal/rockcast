//! RockCast GUI on egui.

use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Layout, Pos2, Rect, RichText, Sense, Stroke, Ui,
    Vec2,
};

use crate::{
    i18n::{self, Lang},
    observers::StreamObservers,
    output::{OutputDevice, scan_streaming},
    playback::{PlaybackController, PlaybackEvent},
    settings::AppSettings,
    spectrum::BANDS,
    stations::{Station, enrich_stations, load_catalog},
};

const BG: Color32 = Color32::from_rgb(0x1a, 0x14, 0x10);
const PANEL: Color32 = Color32::from_rgb(0x24, 0x1c, 0x16);
const PANEL_2: Color32 = Color32::from_rgb(0x2e, 0x24, 0x1c);
const FG: Color32 = Color32::from_rgb(0xe8, 0xdc, 0xc8);
const ACCENT: Color32 = Color32::from_rgb(0xc4, 0x5c, 0x26);
const MUTED: Color32 = Color32::from_rgb(0x9a, 0x8b, 0x78);
const BAR_DIM: Color32 = Color32::from_rgb(0x5a, 0x40, 0x30);
const ROW_H: f32 = 28.0;
/// Slider 100% → 50% on the speaker (half-scale).
enum UiMsg {
    Stations {
        list: Vec<Station>,
        source: String,
        /// false = local catalog (enrich still running), true = final.
        finished: bool,
    },
    DeviceFound(OutputDevice),
    DevicesFinished(String),
    VoiceResult(Result<crate::voice::VoiceSearchResult, String>),
}

fn same_output_device(left: &OutputDevice, right: &OutputDevice) -> bool {
    match (left, right) {
        (OutputDevice::Local(a), OutputDevice::Local(b)) => a.id == b.id,
        (OutputDevice::Cast(a), OutputDevice::Cast(b)) => a.discovered.host == b.discovered.host,
        _ => false,
    }
}

pub struct RockCastApp {
    playback: PlaybackController,
    stations: Vec<Station>,
    devices: Vec<OutputDevice>,
    source: String,
    selected_station: Option<usize>,
    selected_device: Option<usize>,
    status: String,
    station_now: String,
    track: String,
    volume: u8,
    loading_stations: bool,
    loading_devices: bool,
    voice_busy: bool,
    voice_recording: Option<Arc<AtomicBool>>,
    voice_fallback: VecDeque<Station>,
    pending_voice_play: bool,
    /// Cast play/stop running in the background — don't block UI, only update status.
    playing_op: bool,
    playing: bool,
    /// Playing on local speakers (not Cast).
    playing_local: bool,
    playing_url: Option<String>,
    eq_enabled: bool,
    /// Relay station through PC LAN HTTP for Cast (VPN-friendly).
    cast_relay: bool,
    eq_levels: [f32; BANDS],
    eq_peaks: [f32; BANDS],
    observers: StreamObservers,
    ui_rx: mpsc::Receiver<UiMsg>,
    ui_tx: mpsc::Sender<UiMsg>,
    settings: AppSettings,
    last_settings_save: Instant,
    settings_dirty: bool,
    shutting_down: bool,
    bootstrapped: bool,
    lang: Lang,
    rockserver_enabled: bool,
    rockserver_url: String,
    rockserver_bearer_token: String,
}

impl RockCastApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = BG;
        visuals.window_fill = BG;
        visuals.override_text_color = Some(FG);
        visuals.widgets.inactive.bg_fill = PANEL_2;
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x3a, 0x2e, 0x24);
        visuals.widgets.active.bg_fill = ACCENT;
        visuals.selection.bg_fill = ACCENT.gamma_multiply(0.55);
        visuals.extreme_bg_color = PANEL;
        visuals.window_stroke = Stroke::NONE;
        visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3a, 0x2e, 0x24));
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.5));
        cc.egui_ctx.set_visuals(visuals);

        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = Vec2::new(10.0, 8.0);
        style.spacing.button_padding = Vec2::new(14.0, 6.0);
        style.spacing.slider_width = 220.0;
        style.spacing.interact_size.y = 28.0;
        cc.egui_ctx.set_style(style);

        let settings = AppSettings::load();
        let volume = settings.volume.clamp(0, 100);
        let eq_enabled = settings.eq_enabled;
        let cast_relay = settings.cast_relay;
        let lang = settings.language;
        let rockserver_enabled = settings.rockserver_enabled;
        let rockserver_url = settings.rockserver_url.clone();
        let rockserver_bearer_token = settings.rockserver_bearer_token.clone();
        let t = lang.t();

        let (ui_tx, ui_rx) = mpsc::channel();

        Self {
            playback: PlaybackController::new(),
            stations: Vec::new(),
            devices: Vec::new(),
            source: String::new(),
            selected_station: None,
            selected_device: None,
            status: t.loading.into(),
            station_now: "—".into(),
            track: t.track_hint.into(),
            volume,
            loading_stations: false,
            loading_devices: false,
            voice_busy: false,
            voice_recording: None,
            voice_fallback: VecDeque::new(),
            pending_voice_play: false,
            playing_op: false,
            playing: false,
            playing_local: false,
            playing_url: None,
            eq_enabled,
            cast_relay,
            eq_levels: [0.08; BANDS],
            eq_peaks: [0.08; BANDS],
            observers: StreamObservers::new(),
            ui_rx,
            ui_tx,
            settings,
            last_settings_save: Instant::now(),
            settings_dirty: false,
            shutting_down: false,
            bootstrapped: false,
            lang,
            rockserver_enabled,
            rockserver_url,
            rockserver_bearer_token,
        }
    }

    fn restore_station_selection(&mut self) {
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

    fn restore_device_selection(&mut self) {
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

    fn mark_settings_dirty(&mut self) {
        self.settings.volume = self.volume;
        self.settings.eq_enabled = self.eq_enabled;
        self.settings.cast_relay = self.cast_relay;
        self.settings.language = self.lang;
        self.settings.rockserver_enabled = self.rockserver_enabled;
        self.settings.rockserver_url = self.rockserver_url.trim().to_owned();
        self.settings.rockserver_bearer_token = self.rockserver_bearer_token.trim().to_owned();
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

    fn persist_settings_if_needed(&mut self, force: bool) {
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

    fn queue_volume(&self) {
        let is_local = self
            .selected_device
            .and_then(|i| self.devices.get(i))
            .is_some_and(|d| d.is_local())
            || self.playing_local;
        self.playback.set_volume(is_local, self.volume);
    }

    fn shutdown_playback(&mut self) {
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

    fn can_start_play(&self) -> bool {
        !self.loading_devices && !self.shutting_down
    }

    fn bootstrap(&mut self) {
        if self.bootstrapped {
            return;
        }
        self.bootstrapped = true;
        self.refresh_stations();
        self.refresh_devices();
    }

    fn poll_messages(&mut self) {
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
                    if !local {
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
                            log::info!(
                                "voice selected first station: name={:?} url={} fallbacks={}",
                                self.stations[0].name,
                                self.stations[0].url,
                                self.voice_fallback.len()
                            );
                            if result.auto_play {
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
                        Err(error) => self.status = format!("Голосовое управление: {error}"),
                    }
                }
            }
        }
    }

    fn refresh_stations(&mut self) {
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

    fn refresh_devices(&mut self) {
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

    fn start_voice(&mut self) {
        if !self.rockserver_enabled || self.voice_busy {
            return;
        }
        if self.rockserver_bearer_token.trim().is_empty() {
            self.status = "Укажите токен RockServer в настройках.".into();
            return;
        }
        self.voice_busy = true;
        log::info!(
            "voice button pressed: locale=ru-RU rockserver_url={}",
            self.rockserver_url
        );
        self.status = "Слушаю, пока удерживается кнопка…".into();
        let recording = Arc::new(AtomicBool::new(true));
        self.voice_recording = Some(Arc::clone(&recording));
        let tx = self.ui_tx.clone();
        let url = self.rockserver_url.clone();
        let bearer_token = self.rockserver_bearer_token.clone();
        // Voice commands are currently Russian regardless of UI translation.
        let locale = "ru-RU".to_owned();
        let _ = self.playback.spawn_job(move |_| {
            let _ = tx.send(UiMsg::VoiceResult(crate::voice::capture_and_recognize(
                &url,
                &bearer_token,
                &locale,
                recording,
            )));
        });
    }

    fn stop_voice_recording(&mut self) {
        if let Some(recording) = self.voice_recording.take() {
            log::info!("voice button released: committing captured audio");
            recording.store(false, Ordering::Release);
            self.status = "Распознаю команду…".into();
        }
    }

    fn play(&mut self) {
        if !self.can_start_play() {
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
        self.playback
            .play(station, device, self.volume, self.cast_relay && !local);
    }

    fn stop(&mut self) {
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

    fn apply_volume_if_needed(&mut self) {
        // Volume goes out via vol_tx as soon as the slider moves.
        self.persist_settings_if_needed(false);
    }

    fn schedule_stream_tap(&mut self, generation: u64, tap_url: String) {
        self.observers
            .schedule(generation, tap_url, self.cast_relay);
    }

    fn sync_spectrum(&mut self) {
        let tap = if self.cast_relay {
            self.playback
                .relay_public_url()
                .or_else(|| self.playing_url.clone())
        } else {
            self.playing_url.clone()
        };
        let relay_url = self.playback.relay_public_url();
        self.observers.sync(
            self.playing,
            self.playing_local,
            self.eq_enabled,
            tap,
            relay_url.as_deref(),
        );
    }

    fn set_eq_enabled(&mut self, enabled: bool) {
        if self.eq_enabled == enabled {
            return;
        }
        self.eq_enabled = enabled;
        self.mark_settings_dirty();
        self.persist_settings_if_needed(true);
        self.sync_spectrum();
    }

    /// Toggle LAN relay for Cast. If Cast is already playing, restart so the new path applies.
    fn set_cast_relay(&mut self, enabled: bool) {
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

    fn tick_eq(&mut self, dt: f32) {
        let targets = if self.eq_enabled && self.playing {
            if self.playing_local {
                self.playback.local_levels()
            } else {
                self.observers.levels()
            }
        } else {
            [0.08; BANDS]
        };
        for ((level, peak), target) in self
            .eq_levels
            .iter_mut()
            .zip(self.eq_peaks.iter_mut())
            .zip(targets)
        {
            *level += (target - *level) * (dt * 14.0).min(1.0);
            if self.eq_enabled && self.playing {
                *peak = peak.max(*level);
                *peak = (*peak - dt * 0.32).max(*level);
            } else {
                *peak += (0.08 - *peak) * (dt * 10.0).min(1.0);
            }
        }
    }

    fn draw_eq(&self, ui: &mut Ui, size: Vec2) {
        let (rect, _resp) = ui.allocate_exact_size(size, Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(6), PANEL_2);
        let n = self.eq_levels.len();
        let gap = 3.0;
        let bar_w = ((rect.width() - gap * (n as f32 + 1.0)) / n as f32).max(2.0);
        let active = self.eq_enabled && self.playing;
        for (i, level) in self.eq_levels.iter().enumerate() {
            let h = (rect.height() - 8.0) * level.clamp(0.08, 1.0);
            let x0 = rect.left() + gap + i as f32 * (bar_w + gap);
            let y1 = rect.bottom() - 4.0;
            let y0 = y1 - h;
            let color = if active { ACCENT } else { BAR_DIM };
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x0 + bar_w, y1)),
                CornerRadius::same(2),
                color,
            );
            let peak = self.eq_peaks[i].clamp(0.08, 1.0);
            let peak_y = y1 - (rect.height() - 8.0) * peak;
            painter.line_segment(
                [Pos2::new(x0, peak_y), Pos2::new(x0 + bar_w, peak_y)],
                Stroke::new(1.5, if active { Color32::WHITE } else { BAR_DIM }),
            );
        }
    }

    fn set_language(&mut self, ctx: &egui::Context, lang: Lang) {
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

    fn panel(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
        Frame::new()
            .fill(PANEL)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::same(12))
            .show(ui, add);
    }

    fn draw_device_row(&mut self, ui: &mut Ui) {
        let t = self.lang.t();
        let cast_selected = self
            .selected_device
            .and_then(|i| self.devices.get(i))
            .is_some_and(|d| !d.is_local());

        ui.horizontal(|ui| {
            ui.set_height(32.0);
            ui.label(RichText::new(t.device).color(MUTED));
            ui.add_space(8.0);

            let labels: Vec<String> = self
                .devices
                .iter()
                .map(|d| d.label(self.lang))
                .collect();
            let find_w = 88.0;
            // Combo fills remaining width after Find.
            let combo_w = (ui.available_width() - find_w - 10.0).max(160.0);

            let selected_text = match self.selected_device.and_then(|i| labels.get(i)) {
                Some(s) => s.clone(),
                None if labels.is_empty() => {
                    if self.loading_devices {
                        t.searching.into()
                    } else {
                        t.device_none.into()
                    }
                }
                None => t.device_none.into(),
            };

            egui::ComboBox::from_id_salt("device")
                .selected_text(RichText::new(selected_text).color(FG))
                .width(combo_w)
                .height(28.0)
                .show_ui(ui, |ui| {
                    ui.set_min_width(combo_w);
                    if labels.is_empty() {
                        ui.label(RichText::new(t.nothing_found).color(MUTED));
                        return;
                    }
                    for (i, label) in labels.iter().enumerate() {
                        let selected = self.selected_device == Some(i);
                        if ui
                            .selectable_label(selected, RichText::new(label).color(FG))
                            .clicked()
                        {
                            let prev = self.selected_device;
                            self.selected_device = Some(i);
                            if let Some(d) = self.devices.get(i) {
                                let kind = if d.is_local() { "local" } else { "cast" };
                                log::info!(
                                    "output device chosen: idx={i} (was {prev:?}) kind={kind} id={} name='{}'",
                                    d.id(),
                                    d.name()
                                );
                            }
                            self.mark_settings_dirty();
                        }
                    }
                });

            ui.add_space(8.0);
            let find = egui::Button::new(RichText::new(t.find).color(FG))
                .min_size(Vec2::new(find_w, 28.0))
                .fill(PANEL_2);
            if ui
                .add_enabled(!self.loading_devices, find)
                .clicked()
            {
                self.refresh_devices();
            }
        });

        if cast_selected {
            ui.add_space(6.0);
            Frame::new()
                .fill(PANEL_2)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let mut relay = self.cast_relay;
                        let toggle = ui
                            .checkbox(&mut relay, RichText::new(t.cast_relay).color(FG).size(13.0))
                            .on_hover_text(t.cast_relay_hint);
                        if toggle.changed() {
                            self.set_cast_relay(relay);
                        }
                        ui.add_space(10.0);
                        ui.label(RichText::new(t.cast_relay_note).color(MUTED).size(12.0));
                    });
                });
        }
    }

    fn draw_station_list(&mut self, ui: &mut Ui, list_h: f32) {
        let t = self.lang.t();
        ui.horizontal(|ui| {
            ui.label(RichText::new(t.stations).color(FG).size(15.0).strong());
            if !self.source.is_empty() {
                ui.label(
                    RichText::new(format!("· {}", self.source))
                        .color(MUTED)
                        .size(12.0),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let btn = egui::Button::new(RichText::new(t.refresh).color(FG))
                    .fill(PANEL_2)
                    .min_size(Vec2::new(96.0, 28.0));
                if ui.add_enabled(!self.loading_stations, btn).clicked() {
                    self.refresh_stations();
                }
            });
        });
        ui.add_space(4.0);

        let mut should_play = false;
        let mut clicked_station: Option<usize> = None;
        let scroll_h = (list_h - 72.0).max(100.0);
        let col_station = t.col_station;
        let col_tags = t.col_tags;
        let col_bitrate = t.col_bitrate;
        let loading_stations = t.loading_stations;
        let list_empty = t.list_empty;
        let is_loading = self.loading_stations;
        Self::panel(ui, |ui| {
            let full_w = ui.available_width();
            let meta_w = 120.0;
            let tags_w = (full_w * 0.30).clamp(120.0, 240.0);
            let name_w = (full_w - meta_w - tags_w - 24.0).max(140.0);
            let col_name_x = 8.0;
            let col_tags_x = 8.0 + name_w;
            let col_meta_x = 8.0 + name_w + tags_w;

            {
                let (head_rect, _) =
                    ui.allocate_exact_size(Vec2::new(full_w, 20.0), Sense::hover());
                let y = head_rect.center().y;
                ui.painter().text(
                    Pos2::new(head_rect.left() + col_name_x, y),
                    egui::Align2::LEFT_CENTER,
                    col_station,
                    egui::FontId::proportional(12.0),
                    MUTED,
                );
                ui.painter().text(
                    Pos2::new(head_rect.left() + col_tags_x, y),
                    egui::Align2::LEFT_CENTER,
                    col_tags,
                    egui::FontId::proportional(12.0),
                    MUTED,
                );
                ui.painter().text(
                    Pos2::new(head_rect.left() + col_meta_x, y),
                    egui::Align2::LEFT_CENTER,
                    col_bitrate,
                    egui::FontId::proportional(12.0),
                    MUTED,
                );
            }
            ui.add_space(4.0);
            let sep_y = ui.cursor().top();
            ui.painter().hline(
                ui.max_rect().x_range(),
                sep_y,
                Stroke::new(1.0, Color32::from_rgb(0x3a, 0x2e, 0x24)),
            );
            ui.add_space(6.0);

            egui::ScrollArea::vertical()
                .id_salt("stations_scroll")
                .auto_shrink([false, false])
                .max_height(scroll_h)
                .min_scrolled_height(scroll_h)
                .show(ui, |ui| {
                    ui.set_min_width(full_w);
                    if self.stations.is_empty() {
                        ui.allocate_ui_with_layout(
                            Vec2::new(full_w, scroll_h - 8.0),
                            Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                let msg = if is_loading {
                                    loading_stations
                                } else {
                                    list_empty
                                };
                                ui.label(RichText::new(msg).color(MUTED).size(14.0));
                            },
                        );
                        return;
                    }

                    for (i, st) in self.stations.iter().enumerate() {
                        let selected = self.selected_station == Some(i);
                        let meta = [
                            if st.bitrate > 0 {
                                format!("{}k", st.bitrate)
                            } else {
                                String::new()
                            },
                            st.codec.clone(),
                        ]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" / ");
                        let tags = truncate(&st.tags, 42);

                        let (row_rect, resp) =
                            ui.allocate_exact_size(Vec2::new(full_w, ROW_H), Sense::click());
                        if ui.is_rect_visible(row_rect) {
                            let bg = if selected {
                                ACCENT
                            } else if resp.hovered() {
                                PANEL_2
                            } else if i % 2 == 1 {
                                Color32::from_rgb(0x20, 0x18, 0x14)
                            } else {
                                Color32::TRANSPARENT
                            };
                            ui.painter()
                                .rect_filled(row_rect, CornerRadius::same(4), bg);

                            let text_color = if selected { BG } else { FG };
                            let muted_color = if selected {
                                Color32::from_rgb(0x3a, 0x28, 0x1c)
                            } else {
                                MUTED
                            };
                            let y = row_rect.center().y;

                            ui.painter().text(
                                Pos2::new(row_rect.left() + col_name_x, y),
                                egui::Align2::LEFT_CENTER,
                                truncate(&st.name, 48),
                                egui::FontId::proportional(13.5),
                                text_color,
                            );
                            ui.painter().text(
                                Pos2::new(row_rect.left() + col_tags_x, y),
                                egui::Align2::LEFT_CENTER,
                                tags,
                                egui::FontId::proportional(12.5),
                                muted_color,
                            );
                            ui.painter().text(
                                Pos2::new(row_rect.left() + col_meta_x, y),
                                egui::Align2::LEFT_CENTER,
                                meta,
                                egui::FontId::proportional(12.5),
                                muted_color,
                            );
                        }

                        if resp.clicked() {
                            clicked_station = Some(i);
                        }
                        if resp.double_clicked() {
                            clicked_station = Some(i);
                            should_play = true;
                        }
                    }
                });
        });

        if let Some(i) = clicked_station {
            let prev = self.selected_station;
            self.selected_station = Some(i);
            if let Some(s) = self.stations.get(i) {
                log::info!(
                    "station selected: idx={i} (was {prev:?}) name='{}' url={} auto_play={should_play}",
                    s.name,
                    s.url
                );
            }
            self.mark_settings_dirty();
        }
        if should_play {
            log::info!("station double-click → play()");
            self.play();
        }
    }

    fn draw_now_playing(&mut self, ui: &mut Ui) {
        Self::panel(ui, |ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), 72.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.vertical(|ui| {
                        let text_w = (ui.available_width() - 360.0).max(160.0);
                        ui.set_max_width(text_w);
                        ui.label(
                            RichText::new(self.lang.t().now_playing)
                                .color(ACCENT)
                                .strong(),
                        );
                        ui.add_space(2.0);
                        ui.label(RichText::new(&self.station_now).color(MUTED).size(12.5));
                        ui.add_space(2.0);
                        ui.add(
                            egui::Label::new(RichText::new(&self.track).color(FG).size(13.5))
                                .truncate(),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        self.draw_eq(ui, Vec2::new(240.0, 56.0));
                        ui.add_space(10.0);
                        let mut enabled = self.eq_enabled;
                        let toggle = ui
                            .checkbox(
                                &mut enabled,
                                RichText::new(self.lang.t().spectrum).color(FG).size(13.0),
                            )
                            .on_hover_text(self.lang.t().spectrum_hint);
                        if toggle.changed() {
                            self.set_eq_enabled(enabled);
                        }
                    });
                },
            );
        });
    }

    fn draw_controls(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.set_height(34.0);
            ui.label(RichText::new(self.lang.t().volume).color(MUTED));
            ui.add_space(8.0);

            let btn_reserve = 88.0 + 6.0 + 72.0 + 16.0 + 40.0;
            let mut vol = f32::from(self.volume);
            let slider_w = (ui.available_width() - btn_reserve).clamp(120.0, 520.0);
            let slider = egui::Slider::new(&mut vol, 0.0..=100.0)
                .show_value(false)
                .trailing_fill(true);
            if ui.add_sized([slider_w, 18.0], slider).changed() {
                self.volume = vol.round() as u8;
                self.queue_volume();
                self.mark_settings_dirty();
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("{:>3}%", self.volume))
                    .color(FG)
                    .monospace(),
            );

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.rockserver_enabled {
                    let time = ui.input(|input| input.time) as f32;
                    let microphone_active = self.voice_recording.is_some();
                    let voice_color = if microphone_active {
                        let hue = (time * 0.35).fract();
                        Color32::from(egui::ecolor::Hsva::new(hue, 0.92, 1.0, 1.0))
                    } else {
                        PANEL_2
                    };
                    let caption = if self.voice_recording.is_some() {
                        "Слушаю…"
                    } else if self.voice_busy {
                        "Распознаю…"
                    } else {
                        "Голос"
                    };
                    let voice =
                        egui::Button::new(RichText::new(caption).strong().color(Color32::WHITE))
                            .min_size(Vec2::new(72.0, 32.0))
                            .fill(voice_color)
                            .stroke(Stroke::new(2.0, voice_color.gamma_multiply(0.65)));
                    let response = ui.add(voice).on_hover_text(
                        "Удерживайте кнопку и говорите; отпустите для распознавания",
                    );
                    let pressed_on_button = response.is_pointer_button_down_on();
                    let primary_button_down = ui.input(|input| input.pointer.primary_down());
                    if pressed_on_button && !self.voice_busy {
                        self.start_voice();
                    }
                    // Once capture has started, track the physical mouse button globally.
                    // Widget hover/active state can change during animation and must not
                    // terminate a recording while the user still holds the button.
                    if !primary_button_down && self.voice_recording.is_some() {
                        self.stop_voice_recording();
                    }
                    if pressed_on_button || microphone_active {
                        ui.ctx().request_repaint_after(Duration::from_millis(16));
                    }
                    ui.add_space(6.0);
                }
                let stop = egui::Button::new(RichText::new("Stop").color(FG))
                    .min_size(Vec2::new(72.0, 32.0))
                    .fill(PANEL_2);
                if ui.add_enabled(!self.shutting_down, stop).clicked() {
                    log::info!("UI Stop clicked");
                    self.stop();
                }
                ui.add_space(6.0);
                let play = egui::Button::new(RichText::new("Play").strong().color(Color32::WHITE))
                    .min_size(Vec2::new(88.0, 32.0))
                    .fill(ACCENT);
                if ui.add_enabled(self.can_start_play(), play).clicked() {
                    log::info!("UI Play clicked");
                    self.play();
                }
            });
        });
    }

    fn draw_status(&self, ui: &mut Ui) {
        Frame::new()
            .fill(PANEL)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                let status = truncate(&self.status, 120);
                ui.label(RichText::new(status).color(MUTED).size(12.0));
            });
    }
}

impl eframe::App for RockCastApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.bootstrap();
        self.poll_messages();
        self.apply_volume_if_needed();
        if self.playing
            && self.cast_relay
            && !self.playing_local
            && let Some(title) = self.playback.relay_latest_title()
            && !title.is_empty()
            && self.track != title
        {
            self.track = title;
        }
        self.tick_eq(ctx.input(|i| i.stable_dt).clamp(0.0, 0.05));
        let eq_busy = self.eq_enabled && self.playing
            || self.eq_levels.iter().any(|l| (*l - 0.08).abs() > 0.01);
        if self.playing
            || self.loading_stations
            || self.loading_devices
            || self.settings_dirty
            || eq_busy
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        egui::TopBottomPanel::bottom("bottom")
            .frame(
                Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show_separator_line(false)
            .show(ctx, |ui| {
                self.draw_now_playing(ui);
                ui.add_space(8.0);
                self.draw_controls(ui);
                ui.add_space(6.0);
                self.draw_status(ui);
            });

        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(16, 12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("RockCast").size(24.0).color(ACCENT).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let t = self.lang.t();
                        ui.menu_button(
                            RichText::new(t.menu_language).color(MUTED).size(13.0),
                            |ui| {
                                for lang in [Lang::Ru, Lang::En] {
                                    let selected = self.lang == lang;
                                    if ui.selectable_label(selected, lang.native_name()).clicked() {
                                        if self.lang != lang {
                                            self.set_language(ctx, lang);
                                        }
                                        ui.close();
                                    }
                                }
                            },
                        );
                    });
                });
                ui.label(
                    RichText::new(self.lang.t().subtitle)
                        .size(12.5)
                        .color(MUTED),
                );
                ui.add_space(10.0);
                Self::panel(ui, |ui| {
                    ui.horizontal(|ui| {
                        let mut enabled = self.rockserver_enabled;
                        if ui
                            .checkbox(&mut enabled, "RockServer (поиск и голос)")
                            .changed()
                        {
                            self.rockserver_enabled = enabled;
                            self.mark_settings_dirty();
                            self.refresh_stations();
                        }
                        if self.rockserver_enabled {
                            ui.label(RichText::new("URL").color(MUTED));
                            if ui
                                .text_edit_singleline(&mut self.rockserver_url)
                                .lost_focus()
                            {
                                self.mark_settings_dirty();
                            }
                            ui.label(RichText::new("Токен").color(MUTED));
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut self.rockserver_bearer_token)
                                        .password(true),
                                )
                                .lost_focus()
                            {
                                self.mark_settings_dirty();
                            }
                            ui.label(
                                RichText::new(
                                    "Токен сохраняется только в локальных настройках RockCast.",
                                )
                                .color(MUTED)
                                .size(11.0),
                            );
                        } else {
                            ui.label(
                                RichText::new(
                                    "Автономный режим: локальный каталог и Radio Browser",
                                )
                                .color(MUTED),
                            );
                        }
                    });
                });
                ui.add_space(8.0);
                self.draw_device_row(ui);
                ui.add_space(8.0);

                let list_h = (ui.available_height() - 8.0).max(120.0);
                self.draw_station_list(ui, list_h);
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        log::info!("on_exit: shutting down");
        self.shutdown_playback();
        // HTTP decode threads may still be blocked inside reqwest; don't let them
        // keep the process alive after the window is gone.
        std::process::exit(0);
    }
}

impl Drop for RockCastApp {
    fn drop(&mut self) {
        self.shutdown_playback();
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
