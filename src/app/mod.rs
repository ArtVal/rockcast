//! RockCast GUI on egui.

mod actions;
mod messages;
mod theme;
mod ui;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, atomic::AtomicBool, mpsc},
    time::{Duration, Instant},
};

use eframe::egui::{self, Align, Color32, Frame, Layout, RichText, Stroke, TextureHandle, Vec2};

use crate::{
    i18n::Lang,
    observers::{BANDS, StreamObservers},
    output::OutputDevice,
    playback::PlaybackController,
    playback_diag,
    rockserver::RuntimeConfig,
    settings::AppSettings,
    stations::Station,
    telemetry::{PlaybackSnapshot, Telemetry},
};

use messages::UiMsg;
use theme::{ACCENT, BG, EQ_REPAINT_INTERVAL, FG, MUTED, PANEL, PANEL_2, UI_SLOW_REPAINT_INTERVAL};

pub struct RockCastApp {
    pub(super) playback: PlaybackController,
    pub(super) stations: Vec<Station>,
    pub(super) devices: Vec<OutputDevice>,
    pub(super) source: String,
    pub(super) selected_station: Option<usize>,
    pub(super) scroll_to_station: Option<usize>,
    pub(super) station_name_col_w: Option<f32>,
    pub(super) station_tags_col_w: Option<f32>,
    pub(super) selected_device: Option<usize>,
    pub(super) status: String,
    pub(super) station_now: String,
    pub(super) last_played_station: Option<Station>,
    pub(super) personal_data: Option<crate::personal_data::PersonalDataStore>,
    pub(super) favourites_open: bool,
    pub(super) history_open: bool,
    pub(super) track: String,
    pub(super) volume: u8,
    pub(super) loading_stations: bool,
    pub(super) loading_devices: bool,
    pub(super) voice_busy: bool,
    pub(super) voice_recording: Option<Arc<AtomicBool>>,
    pub(super) voice_fallback: VecDeque<Station>,
    pub(super) pending_voice_play: bool,
    /// Cast play/stop running in the background — don't block UI, only update status.
    pub(super) playing_op: bool,
    pub(super) playing: bool,
    /// Playing on local speakers (not Cast).
    pub(super) playing_local: bool,
    pub(super) playing_url: Option<String>,
    pub(super) eq_enabled: bool,
    /// Relay station through PC LAN HTTP for Cast (VPN-friendly).
    pub(super) cast_relay: bool,
    pub(super) eq_levels: [f32; BANDS],
    pub(super) eq_peaks: [f32; BANDS],
    pub(super) observers: StreamObservers,
    pub(super) ui_rx: mpsc::Receiver<UiMsg>,
    pub(super) ui_tx: mpsc::Sender<UiMsg>,
    pub(super) settings: AppSettings,
    pub(super) last_settings_save: Instant,
    pub(super) settings_dirty: bool,
    pub(super) shutting_down: bool,
    pub(super) bootstrapped: bool,
    pub(super) lang: Lang,
    pub(super) rockserver: RuntimeConfig,
    pub(super) telemetry: Telemetry,
    pub(super) eq_repaint_next: Instant,
    /// UI-owned decoded textures. Fetch/decode stays in the BackgroundRuntime.
    pub(super) station_icons: HashMap<String, TextureHandle>,
    /// Request identities already attempted this app session, including failures.
    pub(super) station_icon_requests: HashSet<String>,
    /// Jobs still expected to send a StationIcon message, used to wake egui.
    pub(super) station_icons_pending: usize,
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
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 4.0);
        style.spacing.slider_width = 220.0;
        style.spacing.interact_size.y = 24.0;
        cc.egui_ctx.set_style(style);

        let settings = AppSettings::load();
        let volume = settings.volume.clamp(0, 100);
        let eq_enabled = settings.eq_enabled;
        let cast_relay = settings.cast_relay;
        let lang = settings.language;
        let rockserver = RuntimeConfig::for_app();
        log::info!("RockServer base URL: {}", rockserver.base_url());
        let last_played_station = settings.last_played_station.clone();
        let t = lang.t();

        let (ui_tx, ui_rx) = mpsc::channel();

        Self {
            playback: PlaybackController::new(),
            stations: Vec::new(),
            devices: Vec::new(),
            source: String::new(),
            selected_station: None,
            scroll_to_station: None,
            station_name_col_w: None,
            station_tags_col_w: None,
            selected_device: None,
            status: t.loading.into(),
            station_now: "—".into(),
            last_played_station,
            personal_data: None,
            favourites_open: false,
            history_open: false,
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
            rockserver,
            telemetry: Telemetry::new(),
            eq_repaint_next: Instant::now(),
            station_icons: HashMap::new(),
            station_icon_requests: HashSet::new(),
            station_icons_pending: 0,
        }
    }
    pub(in crate::app) fn can_start_play(&self) -> bool {
        !self.shutting_down
            && self.selected_station.is_some()
            && self.selected_device.is_some()
            && (!self.loading_devices || !self.devices.is_empty())
    }
}

impl eframe::App for RockCastApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.bootstrap();
        self.poll_messages(ctx);
        self.apply_volume_if_needed();
        if self.playing
            && self.playback.relay_active()
            && !self.playing_local
            && let Some(title) = self.playback.relay_latest_title()
            && !title.is_empty()
            && self.track != title
        {
            self.track = title;
        }
        let now = Instant::now();
        let eq_ui_active = self.eq_ui_needs_frames();
        let eq_repaint_due = eq_ui_active && now >= self.eq_repaint_next;
        if eq_repaint_due {
            self.eq_repaint_next = now + EQ_REPAINT_INTERVAL;
            self.tick_eq(EQ_REPAINT_INTERVAL.as_secs_f32());
        }
        self.telemetry.on_frame();
        let needs_fast_repaint = self.voice_recording.is_some() || eq_repaint_due;
        let needs_slow_repaint = self.playing
            || self.playing_op
            || self.loading_stations
            || self.loading_devices
            || self.station_icons_pending > 0
            || self.settings_dirty
            || self.voice_busy;
        let snap = PlaybackSnapshot {
            playing: self.playing,
            eq_enabled: self.eq_enabled,
            cast_relay: self.cast_relay,
            playing_local: self.playing_local,
            fast_repaint: needs_fast_repaint,
        };
        self.telemetry.maybe_log(snap);
        if snap.playing {
            playback_diag::maybe_log(snap);
        }
        if self.voice_recording.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        } else if eq_ui_active {
            let delay = self
                .eq_repaint_next
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(5));
            ctx.request_repaint_after(delay);
        } else if needs_slow_repaint {
            // Keep polling background playback/device work without a full-speed UI loop.
            ctx.request_repaint_after(UI_SLOW_REPAINT_INTERVAL);
        }

        egui::TopBottomPanel::bottom("bottom")
            .frame(
                Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show_separator_line(false)
            .show(ctx, |ui| {
                self.draw_now_playing(ui);
                ui.add_space(6.0);
                self.draw_controls(ui);
                ui.add_space(4.0);
                self.draw_status(ui);
            });

        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("RockCast").size(24.0).color(ACCENT).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let history_count = self.personal_data.as_ref().map_or(0, |store| {
                            store.history().len()
                                + store
                                    .profile()
                                    .unresolved_references
                                    .iter()
                                    .filter(|entry| entry.source_kind == "history")
                                    .count()
                        });
                        if ui.button(format!("History ({history_count})")).clicked() {
                            self.history_open = true;
                        }
                        let favourites_count = self.personal_data.as_ref().map_or(0, |store| {
                            store.favourites().len()
                                + store
                                    .profile()
                                    .unresolved_references
                                    .iter()
                                    .filter(|entry| entry.source_kind == "favourite")
                                    .count()
                        });
                        if ui
                            .button(format!("Favourites ({favourites_count})"))
                            .clicked()
                        {
                            self.favourites_open = true;
                        }
                        let favourite = self
                            .selected_station
                            .and_then(|index| self.stations.get(index))
                            .is_some_and(|station| {
                                self.personal_data
                                    .as_ref()
                                    .is_some_and(|store| store.is_favourite(&station.id))
                            });
                        if ui
                            .add_enabled(
                                self.personal_data.is_some() && self.selected_station.is_some(),
                                egui::Button::new(if favourite {
                                    "★ Favourite"
                                } else {
                                    "☆ Favourite"
                                }),
                            )
                            .clicked()
                        {
                            self.toggle_selected_favourite();
                        }
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
                ui.add_space(8.0);
                self.draw_device_row(ui);
                ui.add_space(6.0);

                let list_h = ui.available_height().max(120.0);
                self.draw_station_list(ui, list_h);
            });
        self.draw_personal_windows(ctx);
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
