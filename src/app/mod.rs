//! RockCast GUI on egui.

mod actions;
mod icons;
mod messages;
mod theme;
mod ui;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use eframe::egui::{self, Color32, Frame, Stroke, TextureHandle, Vec2};

use crate::{
    cast::CastDeviceInfo,
    device_control::{DeviceControlClient, PlayerState, ReceiverCache, playback_status},
    i18n::Lang,
    observers::{BANDS, StreamObservers},
    output::OutputDevice,
    playback::PlaybackController,
    playback_diag,
    rockserver::RuntimeConfig,
    runtime::BackgroundRuntime,
    settings::{AppSettings, SettingsWriter},
    stations::Station,
    telemetry::{PlaybackSnapshot, Telemetry},
};

use messages::UiMsg;
use theme::{
    ACCENT, BG, EQ_REPAINT_INTERVAL, FG, PANEL, PANEL_2, UI_SLOW_REPAINT_INTERVAL,
};

#[derive(Clone)]
pub(super) struct AccountContext {
    pub(super) profile: crate::session::AccountProfile,
    pub(super) devices: Vec<crate::session::Device>,
}

pub(super) enum AccountUiState {
    Disconnected {
        device_name: String,
        message: Option<String>,
    },
    Starting {
        device_name: String,
        loading_account: bool,
    },
    Waiting {
        request: crate::session::PairingRequest,
        status: String,
    },
    ConnectedFirstTime {
        context: AccountContext,
    },
    Connected {
        context: AccountContext,
        banner: Option<String>,
    },
    Error {
        kind: AccountErrorKind,
        cached: Option<AccountContext>,
    },
}

#[derive(Clone, Copy)]
pub(super) enum AccountErrorKind {
    Recoverable,
    SecureStorage,
}

pub(super) struct PendingRemoteCommand {
    pub(super) id: String,
    pub(super) generation: u64,
    pub(super) output: Option<RemoteOutput>,
}

#[derive(Clone)]
pub(super) enum RemoteOutput {
    Local,
    Chromecast(Option<String>),
    Relay(Option<String>),
}

pub(super) fn account_session_active(state: &AccountUiState) -> bool {
    matches!(
        state,
        AccountUiState::Connected { .. } | AccountUiState::ConnectedFirstTime { .. }
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum StationFilterMode {
    #[default]
    All,
    Favourites,
    History,
}

pub struct RockCastApp {
    pub(super) playback: PlaybackController,
    pub(super) background: BackgroundRuntime,
    pub(super) stations: Vec<Station>,
    /// Text sent to the global RockServer station search.
    pub(super) station_search: String,
    pub(super) selected_genre: Option<String>,
    pub(super) filter_mode: StationFilterMode,
    /// Lets the UI discard results from an older global search.
    pub(super) station_request_id: u64,
    pub(super) station_search_total: Option<usize>,
    pub(super) station_search_offset: usize,
    pub(super) station_catalog_cursor: Option<String>,
    pub(super) station_has_more: bool,
    pub(super) loading_more_stations: bool,
    pub(super) loading_more_error: Option<String>,
    pub(super) selected_country: Option<String>,
    pub(super) selected_min_bitrate: Option<u32>,
    pub(super) devices: Vec<OutputDevice>,
    pub(super) source: String,
    pub(super) selected_station: Option<usize>,
    pub(super) scroll_to_station: Option<usize>,
    pub(super) scroll_to_top: bool,
    pub(super) station_name_col_w: Option<f32>,
    pub(super) station_meta_col_w: Option<f32>,
    pub(super) selected_device: Option<usize>,
    pub(super) status: String,
    pub(super) station_now: String,
    pub(super) last_played_station: Option<Station>,
    /// Exact catalog ID of the station chosen for the current playback
    /// lifecycle (local pick or server-resolved command), bound when a start
    /// is actually issued. It survives `error`/`stopped` transitions
    /// (live-control §4.5) and is `None` only before the first choice in this
    /// session; it is never recovered from a stream URL.
    pub(super) playback_station_id: Option<String>,
    pub(super) personal_data: Option<crate::personal_data::PersonalDataStore>,
    /// RM-012-B: per-device favourites/history sync state and UI diagnostics.
    pub(super) sync_state: Option<crate::personal_sync::SyncState>,
    pub(super) sync_status: actions::personal_sync::SyncStatus,
    pub(super) sync_running: bool,
    pub(super) sync_generation: u64,
    pub(super) sync_retry_not_before: Option<Instant>,
    pub(super) sync_backoff_attempts: u32,
    pub(super) sync_debounce_at: Option<Instant>,
    pub(super) sync_last_periodic: Instant,
    /// Favourite stations being resolved from their profile record right now
    /// (RM-012-B follow-up: muted favourite rows are playable).
    pub(super) resolving_stations: HashSet<String>,
    pub(super) failed_resolving_stations: HashSet<String>,
    pub(super) favourites_open: bool,
    pub(super) history_open: bool,
    pub(super) account_open: bool,
    pub(super) pairing_cancel: Option<Arc<AtomicBool>>,
    pub(super) account_state: AccountUiState,
    pub(super) account_load_started: bool,
    pub(super) account_refreshing: bool,
    pub(super) revoke_confirmation: Option<String>,
    pub(super) pairing_link_copied: bool,
    pub(super) track: String,
    pub(super) track_metadata: Option<String>,
    pub(super) volume: u8,
    pub(super) loading_stations: bool,
    pub(super) loading_devices: bool,
    pub(super) voice_busy: bool,
    pub(super) voice_recording: Option<Arc<AtomicBool>>,
    pub(super) voice_fallback: VecDeque<Station>,
    pub(super) voice_search_query: Option<String>,
    /// Query string of the active station search results currently displayed in `stations`.
    /// `None` indicates the default catalog list is active.
    pub(super) active_search_query: Option<String>,
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
    pub(super) settings_writer: SettingsWriter,
    pub(super) last_settings_save: Instant,
    pub(super) settings_dirty: bool,
    pub(super) shutting_down: bool,
    pub(super) bootstrapped: bool,
    pub(super) lang: Lang,
    pub(super) rockserver: RuntimeConfig,
    pub(super) device_control: DeviceControlClient,
    pub(super) pending_remote_command: Option<PendingRemoteCommand>,
    pub(super) pending_chromecast_discovery: Option<String>,
    pub(super) chromecast_receivers: ReceiverCache<CastDeviceInfo>,
    pub(super) output: RemoteOutput,
    pub(super) telemetry: Telemetry,
    pub(super) eq_repaint_next: Instant,
    /// UI-owned decoded textures. Fetch/decode stays in the BackgroundRuntime.
    pub(super) station_icons: HashMap<String, TextureHandle>,
    /// Request identities already attempted this app session, including failures.
    pub(super) station_icon_requests: HashSet<String>,
    /// Jobs still expected to send a StationIcon message, used to wake egui.
    pub(super) station_icons_pending: usize,
    pub(super) app_icons: icons::AppIcons,
    pub(super) frame_count: u64,
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
        // Fits the deck's 196px right section: speaker + slider + percent.
        style.spacing.slider_width = 110.0;
        style.spacing.interact_size.y = 24.0;
        cc.egui_ctx.set_style(style);

        let settings = AppSettings::load();
        let volume = settings.volume.clamp(0, 100);
        let eq_enabled = settings.eq_enabled;
        let cast_relay = settings.cast_relay;
        let lang = settings.language;
        let rockserver = RuntimeConfig::for_app();
        log::info!("RockServer configuration loaded");
        let repaint = cc.egui_ctx.clone();
        let device_control = DeviceControlClient::new(
            rockserver.clone(),
            settings.device_control_state_revision,
            Arc::new(move || repaint.request_repaint()),
        );
        let last_played_station = settings.last_played_station.clone();
        let t = lang.t();

        let (ui_tx, ui_rx) = mpsc::channel();

        Self {
            playback: PlaybackController::new(),
            background: BackgroundRuntime::new_named(4, "rockcast-io"),
            stations: Vec::new(),
            station_search: String::new(),
            selected_genre: None,
            filter_mode: StationFilterMode::All,
            station_request_id: 0,
            station_search_total: None,
            station_search_offset: 0,
            station_catalog_cursor: None,
            station_has_more: false,
            loading_more_stations: false,
            loading_more_error: None,
            selected_country: None,
            selected_min_bitrate: None,
            devices: Vec::new(),
            source: String::new(),
            selected_station: None,
            scroll_to_station: None,
            scroll_to_top: false,
            station_name_col_w: settings.station_name_col_w,
            station_meta_col_w: settings.station_meta_col_w,
            selected_device: None,
            status: t.loading.into(),
            station_now: "—".into(),
            last_played_station,
            playback_station_id: None,
            personal_data: None,
            sync_state: None,
            sync_status: actions::personal_sync::SyncStatus::default(),
            sync_running: false,
            sync_generation: 0,
            sync_retry_not_before: None,
            sync_backoff_attempts: 0,
            sync_debounce_at: None,
            sync_last_periodic: Instant::now(),
            resolving_stations: HashSet::new(),
            failed_resolving_stations: HashSet::new(),
            favourites_open: false,
            history_open: false,
            account_open: false,
            pairing_cancel: None,
            account_state: AccountUiState::Disconnected {
                device_name: default_pairing_device_name(),
                message: None,
            },
            account_load_started: false,
            account_refreshing: false,
            revoke_confirmation: None,
            pairing_link_copied: false,
            track: t.track_hint.into(),
            track_metadata: None,
            volume,
            loading_stations: false,
            loading_devices: false,
            voice_busy: false,
            voice_recording: None,
            voice_fallback: VecDeque::new(),
            voice_search_query: None,
            active_search_query: None,
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
            settings_writer: SettingsWriter::new(),
            last_settings_save: Instant::now(),
            settings_dirty: false,
            shutting_down: false,
            bootstrapped: false,
            lang,
            rockserver,
            device_control,
            pending_remote_command: None,
            pending_chromecast_discovery: None,
            chromecast_receivers: ReceiverCache::default(),
            output: RemoteOutput::Local,
            telemetry: Telemetry::new(),
            eq_repaint_next: Instant::now(),
            station_icons: HashMap::new(),
            station_icon_requests: HashSet::new(),
            station_icons_pending: 0,
            app_icons: icons::AppIcons::new(&cc.egui_ctx),
            frame_count: 0,
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
        self.frame_count = self.frame_count.saturating_add(1);
        if let Ok(shot_path) = std::env::var("ROCKCAST_SCREENSHOT_PATH") {
            let shot_path = shot_path.trim().to_owned();
            let mut captured = false;
            ctx.input(|i| {
                for event in &i.raw.events {
                    if let egui::Event::Screenshot { image, .. } = event {
                        let w = image.size[0] as u32;
                        let h = image.size[1] as u32;
                        let mut pixels = Vec::with_capacity(image.pixels.len() * 4);
                        for p in &image.pixels {
                            pixels.push(p.r());
                            pixels.push(p.g());
                            pixels.push(p.b());
                            pixels.push(255);
                        }
                        let is_blank = image.pixels.iter().all(|p| p.r() <= 10 && p.g() <= 10 && p.b() <= 10);
                        if !is_blank {
                            if let Some(buf) = image::RgbaImage::from_raw(w, h, pixels) {
                                if let Ok(()) = buf.save(&shot_path) {
                                    captured = true;
                                }
                            }
                        }
                    }
                }
            });
            if captured {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            } else if self.frame_count >= 30 && !self.stations.is_empty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                ctx.request_repaint();
            } else {
                ctx.request_repaint();
            }
        }

        self.bootstrap();
        self.poll_messages(ctx);
        self.tick_personal_sync();
        let device_commands_pending = self.poll_device_control_commands();
        self.poll_pairing();
        self.apply_volume_if_needed();
        if self.playing
            && self.playback.relay_active()
            && !self.playing_local
            && let Some(title) = self.playback.relay_latest_title()
            && !title.is_empty()
            && self.track != title
        {
            self.track_metadata = bounded_track_title(&title, &self.station_now);
            self.track = title;
        }
        self.sync_device_control_state();
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
            || self.voice_busy
            || self.account_load_started
            || self.account_refreshing
            || device_commands_pending
            || self.sync_running
            || self.sync_debounce_at.is_some()
            || !self.resolving_stations.is_empty()
            || matches!(self.account_state, AccountUiState::Waiting { .. });
        let snap = PlaybackSnapshot {
            playing: self.playing,
            eq_enabled: self.eq_enabled,
            cast_relay: self.cast_relay,
            playing_local: self.playing_local,
            fast_repaint: needs_fast_repaint,
            sync: self
                .sync_status
                .metric(account_session_active(&self.account_state)),
        };
        self.telemetry.maybe_log(snap);
        if snap.playing {
            playback_diag::maybe_log(snap);
        }
        if let Some(recording) = &self.voice_recording
            && !recording.load(std::sync::atomic::Ordering::Acquire)
        {
            self.voice_recording = None;
            self.status = "Распознаю команду…".into();
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
            .frame(Frame::new().fill(BG).inner_margin(egui::Margin {
                left: 16,
                right: 16,
                top: 6,
                bottom: 12,
            }))
            .show_separator_line(false)
            .show(ctx, |ui| {
                self.draw_player_deck(ui);
            });

        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(egui::Margin {
                left: 16,
                right: 16,
                top: 12,
                bottom: 8,
            }))
            .show(ctx, |ui| {
                self.draw_header(ctx, ui);

                let list_h = ui.available_height().max(120.0);
                self.draw_station_list(ui, list_h);
            });
        self.draw_personal_windows(ctx);
        self.draw_account_window(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        log::info!("on_exit: shutting down");
        self.device_control.shutdown();
        self.shutdown_playback();
        // HTTP decode threads may still be blocked inside reqwest; don't let them
        // keep the process alive after the window is gone.
        std::process::exit(0);
    }
}

fn bounded_track_title(title: &str, station_name: &str) -> Option<String> {
    let title = title.trim().chars().take(256).collect::<String>();
    (!title.is_empty() && title != station_name).then_some(title)
}

impl Drop for RockCastApp {
    fn drop(&mut self) {
        self.device_control.shutdown();
        self.shutdown_playback();
    }
}

impl RockCastApp {
    /// Publishes the owned player's current facts to the device-control slot.
    ///
    /// The phase machine is the single source of playback status (§4.3) and
    /// the station chosen for the current lifecycle supplies `station_id`
    /// through buffering, playing, error and stopped (§4.5). Equal states are
    /// deduplicated by [`DeviceControlClient::publish`]; only a changed fact
    /// advances the persisted revision.
    fn sync_device_control_state(&mut self) {
        let station_id = self.playback_station_id.clone();
        let state = PlayerState {
            playback_status: playback_status(self.playback.phase(), station_id.as_deref()),
            station_id,
            track_title: self.track_metadata.clone(),
            volume: self.volume,
            output_mode: match &self.output {
                RemoteOutput::Local => "local",
                RemoteOutput::Chromecast(_) => "chromecast",
                RemoteOutput::Relay(_) => "relay",
            },
            receiver_id: match &self.output {
                RemoteOutput::Chromecast(receiver_id) | RemoteOutput::Relay(receiver_id) => {
                    receiver_id.clone()
                }
                RemoteOutput::Local => None,
            },
        };
        if let Some(revision) = self.device_control.publish(state) {
            self.settings.device_control_state_revision = revision;
            self.settings_dirty = true;
        }
    }

    fn poll_pairing(&mut self) {
        let AccountUiState::Waiting {
            request: pairing, ..
        } = &self.account_state
        else {
            return;
        };
        let pairing = pairing.clone();
        if self.pairing_cancel.is_some() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.pairing_cancel = Some(Arc::clone(&cancel));
        let tx = self.ui_tx.clone();
        let rockserver = self.rockserver.clone();
        if self
            .background
            .spawn(move |_| {
                let client = crate::session::AccountClient::new(
                    rockserver,
                    crate::session::OsCredentialStore,
                );
                let deadline = Instant::now() + Duration::from_secs(10 * 60);
                loop {
                    match crate::session::pairing_poll_control(
                        cancel.load(Ordering::Relaxed),
                        Instant::now(),
                        deadline,
                    ) {
                        crate::session::PairingPollControl::Continue => {}
                        crate::session::PairingPollControl::Cancelled => return,
                        crate::session::PairingPollControl::TimedOut => {
                            let _ = tx.send(UiMsg::PairingResult {
                                request_id: pairing.pairing_request_id.clone(),
                                result: Err(crate::session::PairingPoll::TimedOut),
                            });
                            return;
                        }
                    }
                    match client.complete_pairing_result(&pairing) {
                        Ok((profile, credentials)) => {
                            if cancel.load(Ordering::Relaxed) {
                                return;
                            }
                            if let Err(reason) = client.save_pairing_credentials(&credentials) {
                                let _ = tx.send(UiMsg::PairingResult {
                                    request_id: pairing.pairing_request_id.clone(),
                                    result: Err(reason),
                                });
                                return;
                            }
                            if !client.has_credentials().unwrap_or(false) {
                                log::error!("pairing credentials were not persisted locally");
                                let _ = tx.send(UiMsg::PairingResult {
                                    request_id: pairing.pairing_request_id.clone(),
                                    result: Err(
                                        crate::session::PairingPoll::SecureStorageUnavailable,
                                    ),
                                });
                                return;
                            }
                            let _ = tx.send(UiMsg::PairingResult {
                                request_id: pairing.pairing_request_id.clone(),
                                result: Ok(profile),
                            });
                            return;
                        }
                        Err(crate::session::PairingPoll::Pending) => {
                            std::thread::sleep(Duration::from_secs(2));
                        }
                        Err(crate::session::PairingPoll::Unavailable) => {
                            let _ = tx.send(UiMsg::PairingResult {
                                request_id: pairing.pairing_request_id.clone(),
                                result: Err(crate::session::PairingPoll::Unavailable),
                            });
                            return;
                        }
                        Err(reason) => {
                            let _ = tx.send(UiMsg::PairingResult {
                                request_id: pairing.pairing_request_id.clone(),
                                result: Err(reason),
                            });
                            return;
                        }
                    }
                }
            })
            .is_err()
        {
            self.pairing_cancel = None;
            self.account_state = AccountUiState::Error {
                kind: AccountErrorKind::Recoverable,
                cached: None,
            };
        }
    }
}

pub(in crate::app) fn default_pairing_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "This PC".into())
}

#[cfg(test)]
mod track_title_tests {
    use super::bounded_track_title;

    #[test]
    fn only_real_track_metadata_is_published() {
        assert_eq!(
            bounded_track_title("Artist - Track", "Radio"),
            Some("Artist - Track".into())
        );
        assert_eq!(bounded_track_title("Radio", "Radio"), None);
        assert_eq!(bounded_track_title("  ", "Radio"), None);
    }
}
