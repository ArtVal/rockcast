//! RockCast GUI on egui.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Layout, Pos2, Rect, RichText, Sense, Stroke, Ui,
    Vec2,
};

use crate::{
    cast::CastService,
    i18n::{self, Lang},
    icy::IcyWatcher,
    local::LocalPlayer,
    output::{OutputDevice, scan_all},
    relay::StreamRelay,
    settings::AppSettings,
    spectrum::{BANDS, SpectrumAnalyzer},
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
const VOLUME_CAST_SCALE: f32 = 0.5;

fn ui_volume_to_cast(ui_percent: u8) -> f32 {
    (f32::from(ui_percent) / 100.0 * VOLUME_CAST_SCALE).clamp(0.0, VOLUME_CAST_SCALE)
}

fn ui_volume_to_local(ui_percent: u8) -> f32 {
    (f32::from(ui_percent) / 100.0).clamp(0.0, 1.0)
}

enum UiMsg {
    Status(String),
    Stations {
        list: Vec<Station>,
        source: String,
        /// false = local catalog (enrich still running), true = final.
        finished: bool,
    },
    Devices(Vec<OutputDevice>, String),
    PlayOk {
        url: String,
        /// HTTP URL for spectrum/ICY tap (relay LAN URL when Via PC).
        tap_url: String,
        generation: u64,
    },
    StartTap { url: String, generation: u64 },
    StopOk,
    Error { message: String, generation: Option<u64> },
    Track(String),
}

pub struct RockCastApp {
    cast: Arc<CastService>,
    local: Arc<LocalPlayer>,
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
    /// Cast play/stop running in the background — don't block UI, only update status.
    playing_op: bool,
    playing: bool,
    /// Playing on local speakers (not Cast).
    playing_local: bool,
    play_generation: Arc<AtomicU64>,
    playing_url: Option<String>,
    eq_enabled: bool,
    /// Relay station through PC LAN HTTP for Cast (VPN-friendly).
    cast_relay: bool,
    eq_levels: [f32; BANDS],
    spectrum: SpectrumAnalyzer,
    ui_rx: mpsc::Receiver<UiMsg>,
    ui_tx: mpsc::Sender<UiMsg>,
    icy: IcyWatcher,
    relay: Arc<StreamRelay>,
    settings: AppSettings,
    /// (is_local, level 0..1)
    vol_tx: mpsc::Sender<(bool, f32)>,
    last_settings_save: Instant,
    settings_dirty: bool,
    shutting_down: bool,
    bootstrapped: bool,
    lang: Lang,
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
        let t = lang.t();

        let (ui_tx, ui_rx) = mpsc::channel();
        let cast = Arc::new(CastService::new());
        let local = Arc::new(LocalPlayer::new());
        let relay = Arc::new(StreamRelay::new());

        let (vol_tx, vol_rx) = mpsc::channel::<(bool, f32)>();
        let cast_vol = Arc::clone(&cast);
        let local_vol = Arc::clone(&local);
        thread::spawn(move || {
            while let Ok(first) = vol_rx.recv() {
                let mut target = first;
                while let Ok(newer) = vol_rx.try_recv() {
                    target = newer;
                }
                let (is_local, level) = target;
                if is_local {
                    local_vol.set_volume(level);
                } else {
                    let _ = cast_vol.set_volume_current(level);
                }
            }
        });

        Self {
            cast,
            local,
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
            playing_op: false,
            playing: false,
            playing_local: false,
            play_generation: Arc::new(AtomicU64::new(0)),
            playing_url: None,
            eq_enabled,
            cast_relay,
            eq_levels: [0.08; BANDS],
            spectrum: SpectrumAnalyzer::new(),
            ui_rx,
            ui_tx,
            icy: IcyWatcher::new(),
            relay,
            settings,
            vol_tx,
            last_settings_save: Instant::now(),
            settings_dirty: false,
            shutting_down: false,
            bootstrapped: false,
            lang,
        }
    }

    fn restore_station_selection(&mut self) {
        if let Some(url) = self.settings.station_url.as_ref() {
            if let Some(i) = self.stations.iter().position(|s| &s.url == url) {
                self.selected_station = Some(i);
                return;
            }
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
        if let Some(id) = self.settings.device_id.as_ref() {
            if let Some(i) = self.devices.iter().position(|d| d.id() == id) {
                self.selected_device = Some(i);
                return;
            }
        }
        let mut pick = 0;
        for (i, d) in self.devices.iter().enumerate() {
            if let Some(c) = d.as_cast() {
                let blob =
                    format!("{} {}", c.discovered.name, c.discovered.model).to_lowercase();
                if blob.contains("jbl") || blob.contains("9.1") || blob.contains("bar") {
                    self.selected_device = Some(i);
                    return;
                }
                if pick == 0 && !d.is_local() {
                    pick = i;
                }
            }
        }
        // Prefer any Cast over the first local when nothing was saved.
        if pick == 0 {
            if let Some(i) = self.devices.iter().position(|d| !d.is_local()) {
                pick = i;
            }
        }
        self.selected_device = Some(pick);
    }

    fn mark_settings_dirty(&mut self) {
        self.settings.volume = self.volume;
        self.settings.eq_enabled = self.eq_enabled;
        self.settings.cast_relay = self.cast_relay;
        self.settings.language = self.lang;
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
        self.settings.save();
    }

    fn queue_volume(&self) {
        let is_local = self
            .selected_device
            .and_then(|i| self.devices.get(i))
            .is_some_and(|d| d.is_local())
            || self.playing_local;
        let level = if is_local {
            ui_volume_to_local(self.volume)
        } else {
            ui_volume_to_cast(self.volume)
        };
        let _ = self.vol_tx.send((is_local, level));
    }

    fn shutdown_playback(&mut self) {
        if self.shutting_down {
            return;
        }
        let generation = self.play_generation.fetch_add(1, Ordering::SeqCst) + 1;
        log::info!(
            "shutdown_playback: bump generation→{generation} playing={} local={}",
            self.playing,
            self.playing_local
        );
        self.shutting_down = true;
        self.icy.stop_async();
        self.spectrum.stop_async();
        self.playing = false;
        self.playing_local = false;
        self.playing_url = None;
        self.mark_settings_dirty();
        self.persist_settings_if_needed(true);
        // Stop local first (non-blocking). Cast STOP is best-effort with a short wait
        // so a hung Cast handshake cannot freeze window close.
        log::info!("shutdown_playback: local.stop()");
        self.local.stop();
        self.relay.stop();
        let cast = Arc::clone(&self.cast);
        let done = Arc::new(AtomicBool::new(false));
        let done_flag = Arc::clone(&done);
        thread::spawn(move || {
            log::info!("shutdown_playback: cast.stop() begin");
            let _ = cast.stop();
            done_flag.store(true, Ordering::SeqCst);
            log::info!("shutdown_playback: cast.stop() done");
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !done.load(Ordering::SeqCst) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        if !done.load(Ordering::SeqCst) {
            log::warn!("shutdown_playback: cast.stop() still pending after 2s — continuing");
        }
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
        while let Ok(msg) = self.ui_rx.try_recv() {
            match msg {
                UiMsg::Status(s) => self.status = s,
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
                UiMsg::Devices(list, status) => {
                    log::info!(
                        "devices scanned: count={} status={status}",
                        list.len()
                    );
                    for (i, d) in list.iter().enumerate() {
                        let kind = if d.is_local() { "local" } else { "cast" };
                        log::info!(
                            "  device[{i}] kind={kind} id={} name='{}'",
                            d.id(),
                            d.name()
                        );
                    }
                    self.devices = list;
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
                UiMsg::PlayOk {
                    url,
                    tap_url,
                    generation,
                } => {
                    let cur = self.play_generation.load(Ordering::SeqCst);
                    if generation != cur {
                        log::info!(
                            "PlayOk ignored: stale generation={generation} current={cur} url={url}"
                        );
                        continue;
                    }
                    log::info!(
                        "PlayOk: generation={generation} local={} url={url} tap={tap_url}",
                        self.playing_local
                    );
                    self.playing_op = false;
                    self.playing = true;
                    self.playing_url = Some(url);
                    self.track = self.lang.t().track_meta_hint.into();
                    if self.playing_local {
                        // Titles and spectrum already come from LocalPlayer.
                    } else {
                        // Relay tap shares the feeder (no second upstream). Direct Cast
                        // still taps the station URL after a short delay.
                        self.schedule_stream_tap(generation, tap_url);
                    }
                }
                UiMsg::StartTap { url, generation } => {
                    if generation != self.play_generation.load(Ordering::SeqCst) || !self.playing {
                        continue;
                    }
                    self.start_stream_tap(url, self.eq_enabled);
                }
                UiMsg::StopOk => {
                    log::info!("StopOk received");
                    self.playing_op = false;
                    self.playing = false;
                    self.playing_local = false;
                    self.playing_url = None;
                    self.icy.stop_async();
                    self.spectrum.stop_async();
                    self.track = self.lang.t().stopped.into();
                    self.status = self.lang.t().stopped.into();
                }
                UiMsg::Error { message, generation } => {
                    let cur = self.play_generation.load(Ordering::SeqCst);
                    if let Some(g) = generation
                        && g != cur
                    {
                        log::info!(
                            "Error ignored: stale generation={g} current={cur} msg={message}"
                        );
                        continue;
                    }
                    log::error!(
                        "Error applied: generation={generation:?} current={cur} msg={message}"
                    );
                    self.playing_op = false;
                    self.loading_stations = false;
                    self.loading_devices = false;
                    self.playing = false;
                    self.playing_local = false;
                    self.playing_url = None;
                    self.icy.stop_async();
                    self.spectrum.stop_async();
                    self.local.stop();
                    self.station_now = "—".into();
                    self.track = self.lang.t().track_hint.into();
                    self.status = message.clone();
                }
                UiMsg::Track(t) => self.track = t,
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
        thread::spawn(move || {
            let (catalog, source) = load_catalog(lang);
            let _ = tx.send(UiMsg::Stations {
                list: catalog.clone(),
                source,
                finished: false,
            });
            let (merged, source) = enrich_stations(catalog, lang);
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
        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                scan_all(Duration::from_secs(10), lang)
            }));
            match result {
                Ok((list, status)) => {
                    let _ = tx.send(UiMsg::Devices(list, status));
                }
                Err(_) => {
                    let _ = tx.send(UiMsg::Devices(
                        Vec::new(),
                        lang.t().scan_panic.into(),
                    ));
                }
            }
        });
    }

    fn play(&mut self) {
        if !self.can_start_play() {
            log::warn!(
                "play skipped: can_start_play=false loading_devices={} shutting_down={}",
                self.loading_devices,
                self.shutting_down
            );
            return;
        }
        let Some(si) = self.selected_station else {
            self.status = self.lang.t().pick_station.into();
            log::warn!("play skipped: no station selected");
            return;
        };
        let Some(di) = self.selected_device else {
            self.status = self.lang.t().pick_device.into();
            log::warn!("play skipped: no device selected");
            return;
        };
        let station = self.stations[si].clone();
        let device = self.devices[di].clone();
        let is_local = device.is_local();
        let output_kind = if is_local { "local" } else { "cast" };

        log::info!(
            "play request: station='{}' idx={si} url={} → device='{}' id={} kind={output_kind} relay={} vol={}",
            station.name,
            station.url,
            device.name(),
            device.id(),
            self.cast_relay && !is_local,
            self.volume
        );

        // A new station immediately cancels waiting for the previous one's metadata.
        self.icy.stop_async();
        self.spectrum.stop_async();
        if !is_local {
            log::info!("play: stopping local before cast");
            self.local.stop();
        }
        self.playing_url = None;
        let generation = self.play_generation.fetch_add(1, Ordering::SeqCst) + 1;
        log::info!("play: generation={generation}");

        self.playing_op = true;
        self.playing = false;
        self.playing_local = is_local;
        self.status = format!("Play: {} -> {}", station.name, device.name());
        self.station_now = station.name.clone();
        self.track = self.lang.t().connecting.into();
        self.mark_settings_dirty();
        self.persist_settings_if_needed(true);

        let tx = self.ui_tx.clone();
        let cast = Arc::clone(&self.cast);
        let local = Arc::clone(&self.local);
        let relay = Arc::clone(&self.relay);
        let play_generation = Arc::clone(&self.play_generation);
        let vol = self.volume;
        let use_relay = self.cast_relay;

        thread::spawn(move || {
            let cur = play_generation.load(Ordering::SeqCst);
            if cur != generation {
                log::info!(
                    "play worker exit early: generation={generation} superseded by {cur}"
                );
                return;
            }

            match device {
                OutputDevice::Cast(cast_dev) => {
                    log::info!(
                        "play worker[{generation}]: cast path device='{}' relay={use_relay}",
                        cast_dev.discovered.name
                    );
                    // Stop the local player if it was running.
                    local.stop();
                    relay.stop();
                    if play_generation.load(Ordering::SeqCst) != generation {
                        log::info!(
                            "play worker[{generation}]: superseded before cast.play"
                        );
                        return;
                    }

                    let cancel = Arc::new(AtomicBool::new(false));
                    {
                        let cancel_w = Arc::clone(&cancel);
                        let play_generation = Arc::clone(&play_generation);
                        thread::spawn(move || {
                            while !cancel_w.load(Ordering::SeqCst) {
                                if play_generation.load(Ordering::SeqCst) != generation {
                                    cancel_w.store(true, Ordering::SeqCst);
                                    break;
                                }
                                thread::sleep(Duration::from_millis(100));
                            }
                        });
                    }

                    let (load_url, load_ct) = if use_relay {
                        match relay.start(
                            &station.url,
                            &cast_dev.discovered.host,
                            station.content_type(),
                            &cancel,
                        ) {
                            Ok(pair) => {
                                cancel.store(true, Ordering::SeqCst);
                                log::info!(
                                    "play worker[{generation}]: relay url={} content-type={}",
                                    pair.0,
                                    pair.1
                                );
                                pair
                            }
                            Err(e) => {
                                cancel.store(true, Ordering::SeqCst);
                                log::error!(
                                    "play worker[{generation}]: relay.start Err: {e}"
                                );
                                let _ = tx.send(UiMsg::Error {
                                    message: e.to_string(),
                                    generation: Some(generation),
                                });
                                return;
                            }
                        }
                    } else {
                        cancel.store(true, Ordering::SeqCst);
                        (
                            station.url.clone(),
                            station.content_type().to_string(),
                        )
                    };

                    if play_generation.load(Ordering::SeqCst) != generation {
                        log::info!(
                            "play worker[{generation}]: superseded after relay.start"
                        );
                        relay.stop();
                        return;
                    }

                    let play_result = cast.play(
                        &cast_dev,
                        &load_url,
                        &load_ct,
                        &station.name,
                        |s| {
                            if play_generation.load(Ordering::SeqCst) == generation {
                                let _ = tx.send(UiMsg::Status(s.to_string()));
                            }
                        },
                    );
                    if play_generation.load(Ordering::SeqCst) != generation {
                        log::info!(
                            "play worker[{generation}]: superseded after cast.play (stale result dropped)"
                        );
                        relay.stop();
                        return;
                    }
                    match play_result {
                        Ok(()) => {
                            log::info!("play worker[{generation}]: cast.play Ok");
                            let _ = cast.set_volume_current(ui_volume_to_cast(vol));
                            if play_generation.load(Ordering::SeqCst) != generation {
                                relay.stop();
                                return;
                            }
                            if use_relay {
                                if let Some(u) = relay.public_url() {
                                    let _ = tx.send(UiMsg::Status(format!(
                                        "Playing via PC relay ({u})"
                                    )));
                                }
                            }
                            let tap_url = if use_relay {
                                relay
                                    .public_url()
                                    .unwrap_or_else(|| station.url.clone())
                            } else {
                                station.url.clone()
                            };
                            let _ = tx.send(UiMsg::PlayOk {
                                url: station.url.clone(),
                                tap_url,
                                generation,
                            });
                        }
                        Err(e) => {
                            log::error!("play worker[{generation}]: cast.play Err: {e}");
                            relay.stop();
                            let _ = tx.send(UiMsg::Error {
                                message: e.to_string(),
                                generation: Some(generation),
                            });
                        }
                    }
                }
                OutputDevice::Local(local_dev) => {
                    log::info!(
                        "play worker[{generation}]: local path device='{}' cpal={:?}",
                        local_dev.name,
                        local_dev.cpal_name
                    );
                    relay.stop();
                    let _ = cast.stop();

                    let (title_tx, title_rx) = mpsc::channel();
                    let ui_tx = tx.clone();
                    thread::spawn(move || {
                        while let Ok(title) = title_rx.recv() {
                            let _ = ui_tx.send(UiMsg::Track(title));
                        }
                    });

                    if play_generation.load(Ordering::SeqCst) != generation {
                        log::info!(
                            "play worker[{generation}]: superseded before local.play"
                        );
                        return;
                    }
                    let play_result = local.play(
                        &local_dev,
                        &station.url,
                        ui_volume_to_local(vol),
                        Some(title_tx),
                        |s| {
                            if play_generation.load(Ordering::SeqCst) == generation {
                                let _ = tx.send(UiMsg::Status(s.to_string()));
                            }
                        },
                    );
                    // Stale play must NOT call local.stop() — that kills the newer station.
                    let cur = play_generation.load(Ordering::SeqCst);
                    if cur != generation {
                        log::info!(
                            "play worker[{generation}]: superseded after local.play (current={cur}, result={play_result:?}) — not stopping"
                        );
                        return;
                    }
                    match play_result {
                        Ok(()) => {
                            log::info!("play worker[{generation}]: local.play Ok");
                            let _ = tx.send(UiMsg::PlayOk {
                                url: station.url.clone(),
                                tap_url: station.url.clone(),
                                generation,
                            });
                        }
                        Err(e) => {
                            log::error!("play worker[{generation}]: local.play Err: {e}");
                            let _ = tx.send(UiMsg::Error {
                                message: e.to_string(),
                                generation: Some(generation),
                            });
                        }
                    }
                }
            }
        });
    }

    fn stop(&mut self) {
        if self.shutting_down {
            log::warn!("stop skipped: shutting_down");
            return;
        }
        // Cancel any in-flight Play.
        let generation = self.play_generation.fetch_add(1, Ordering::SeqCst) + 1;
        log::info!(
            "stop request: generation→{generation} was_playing={} local={}",
            self.playing,
            self.playing_local
        );
        self.playing_op = true;
        self.status = "Stop…".into();
        self.icy.stop_async();
        self.spectrum.stop_async();
        self.playing = false;
        self.playing_local = false;
        self.playing_url = None;
        self.track = self.lang.t().stopped.into();
        let tx = self.ui_tx.clone();
        let cast = Arc::clone(&self.cast);
        let local = Arc::clone(&self.local);
        let relay = Arc::clone(&self.relay);
        thread::spawn(move || {
            log::info!("stop worker: local.stop()");
            local.stop();
            relay.stop();
            match cast.stop() {
                Ok(()) => {
                    log::info!("stop worker: cast.stop Ok");
                    let _ = tx.send(UiMsg::StopOk);
                }
                Err(e) => {
                    log::error!("stop worker: cast.stop Err: {e}");
                    let _ = tx.send(UiMsg::Error {
                        message: e.to_string(),
                        generation: None,
                    });
                }
            }
        });
    }

    fn apply_volume_if_needed(&mut self) {
        // Volume goes out via vol_tx as soon as the slider moves.
        self.persist_settings_if_needed(false);
    }

    fn schedule_stream_tap(&mut self, generation: u64, tap_url: String) {
        self.icy.stop_async();
        self.spectrum.stop_async();

        let play_generation = Arc::clone(&self.play_generation);
        let ui_tx = self.ui_tx.clone();
        // Via PC: tap the relay URL (shared feeder) shortly after Cast connects.
        // Direct Cast: wait longer so the speaker claims the station first.
        let delay_ms = if self.cast_relay { 2_500 } else { 1_200 };

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(delay_ms));
            if play_generation.load(Ordering::SeqCst) != generation {
                return;
            }
            let _ = ui_tx.send(UiMsg::StartTap {
                url: tap_url,
                generation,
            });
        });
    }

    fn start_stream_tap(&mut self, url: String, eq_enabled: bool) {
        self.icy.stop_async();
        self.spectrum.stop_async();

        // Relay already publishes ICY titles via StreamRelay::latest_title().
        let from_relay = self.relay.public_url().as_ref() == Some(&url);

        let title_tx = if from_relay {
            None
        } else {
            let (tx, rx) = mpsc::channel();
            let ui_tx = self.ui_tx.clone();
            thread::spawn(move || {
                while let Ok(title) = rx.recv() {
                    let _ = ui_tx.send(UiMsg::Track(title));
                }
            });
            Some(tx)
        };

        if eq_enabled {
            self.spectrum.start(url, title_tx);
        } else if let Some(tx) = title_tx {
            self.icy.start(url, tx);
        }
    }

    fn sync_spectrum(&mut self) {
        if self.playing_local {
            // Spectrum is computed in LocalPlayer; a separate HTTP tap is not needed.
            self.spectrum.stop_async();
            self.icy.stop_async();
            return;
        }
        if !self.playing {
            self.spectrum.stop_async();
            self.icy.stop_async();
            return;
        }
        let tap = if self.cast_relay {
            self.relay
                .public_url()
                .or_else(|| self.playing_url.clone())
        } else {
            self.playing_url.clone()
        };
        let Some(url) = tap else {
            self.spectrum.stop_async();
            self.icy.stop_async();
            return;
        };
        self.start_stream_tap(url, self.eq_enabled);
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
                self.local.levels()
            } else {
                self.spectrum.levels()
            }
        } else {
            [0.08; BANDS]
        };
        for (level, target) in self.eq_levels.iter_mut().zip(targets) {
            *level += (target - *level) * (dt * 14.0).min(1.0);
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
                            .checkbox(
                                &mut relay,
                                RichText::new(t.cast_relay).color(FG).size(13.0),
                            )
                            .on_hover_text(t.cast_relay_hint);
                        if toggle.changed() {
                            self.set_cast_relay(relay);
                        }
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(t.cast_relay_note)
                                .color(MUTED)
                                .size(12.0),
                        );
                    });
                });
        }
    }

    fn draw_station_list(&mut self, ui: &mut Ui, list_h: f32) {
        let t = self.lang.t();
        ui.horizontal(|ui| {
            ui.label(RichText::new(t.stations).color(FG).size(15.0).strong());
            if !self.source.is_empty() {
                ui.label(RichText::new(format!("· {}", self.source)).color(MUTED).size(12.0));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let btn = egui::Button::new(RichText::new(t.refresh).color(FG))
                    .fill(PANEL_2)
                    .min_size(Vec2::new(96.0, 28.0));
                if ui
                    .add_enabled(!self.loading_stations, btn)
                    .clicked()
                {
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
                        ui.label(RichText::new(self.lang.t().now_playing).color(ACCENT).strong());
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
        if self.playing && self.cast_relay && !self.playing_local {
            if let Some(title) = self.relay.latest_title() {
                if !title.is_empty() && self.track != title {
                    self.track = title;
                }
            }
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
            ctx.request_repaint_after(Duration::from_millis(33));
        }

        egui::TopBottomPanel::bottom("bottom")
            .frame(Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(16, 10)))
            .show_separator_line(false)
            .show(ctx, |ui| {
                self.draw_now_playing(ui);
                ui.add_space(8.0);
                self.draw_controls(ui);
                ui.add_space(6.0);
                self.draw_status(ui);
            });

        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(16, 12)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("RockCast").size(24.0).color(ACCENT).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let t = self.lang.t();
                        ui.menu_button(RichText::new(t.menu_language).color(MUTED).size(13.0), |ui| {
                            for lang in [Lang::Ru, Lang::En] {
                                let selected = self.lang == lang;
                                if ui
                                    .selectable_label(selected, lang.native_name())
                                    .clicked()
                                {
                                    if self.lang != lang {
                                        self.set_language(ctx, lang);
                                    }
                                    ui.close();
                                }
                            }
                        });
                    });
                });
                ui.label(
                    RichText::new(self.lang.t().subtitle)
                        .size(12.5)
                        .color(MUTED),
                );
                ui.add_space(10.0);
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
