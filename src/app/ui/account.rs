use super::super::theme::{
    BORDER, FG, FS_BODY, FS_MICRO, FS_ROW, FS_SMALL, FS_TITLE, GREEN, MUTED, PANEL_2,
};
use super::super::{
    AccountContext, AccountErrorKind, AccountUiState, RockCastApp, account_session_active,
};
use crate::{
    i18n,
    session::{AccountClient, OsCredentialStore},
};
use eframe::egui::{
    self, Align, Align2, Color32, Context, CornerRadius, FontId, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, Vec2,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const DANGER: Color32 = Color32::from_rgb(0xff, 0x7b, 0x72);

fn danger_dim() -> Color32 {
    Color32::from_rgba_unmultiplied(255, 123, 114, 90)
}

impl RockCastApp {
    /// RM-012-B sync diagnostics line for the account panel; phase and time
    /// only, never identifiers or payloads.
    fn draw_sync_status(&self, ui: &mut egui::Ui, t: &i18n::Strings) {
        use super::super::actions::personal_sync::SyncPhase;
        let state = match self.sync_status.phase {
            SyncPhase::Ok => match &self.sync_status.last_sync_at {
                Some(at) => format!("{} · {}", t.sync_state_ok, at),
                None => t.sync_state_ok.into(),
            },
            SyncPhase::Error => t.sync_state_error.into(),
            SyncPhase::Idle => t.sync_state_idle.into(),
        };
        ui.label(
            RichText::new(format!("{}: {}", t.sync_title, state))
                .color(MUTED)
                .size(FS_SMALL),
        );
    }

    pub(in crate::app) fn ensure_account_loaded(&mut self) {
        if self.account_load_started {
            return;
        }
        self.account_load_started = true;
        self.spawn_account_load();
    }

    fn begin_account_load(&mut self) {
        if matches!(self.account_state, AccountUiState::Waiting { .. }) {
            return;
        }
        if account_session_active(&self.account_state) {
            return;
        }
        if !self.account_load_started {
            return;
        }
        if !matches!(self.account_state, AccountUiState::Starting { .. }) {
            self.account_state = AccountUiState::Starting {
                device_name: default_device_name(),
                loading_account: true,
            };
        }
    }

    pub(in crate::app) fn force_account_reload(&mut self) {
        self.account_refreshing = false;
        self.account_load_started = true;
        self.spawn_account_load();
    }

    pub(in crate::app) fn refresh_account(&mut self) {
        if self.account_load_started || self.account_refreshing {
            return;
        }
        self.account_refreshing = true;
        self.account_load_started = true;
        if let AccountUiState::Connected { banner, .. } = &mut self.account_state {
            *banner = Some(self.lang.t().account_checking.into());
        }
        self.spawn_account_load();
    }

    fn spawn_account_load(&mut self) {
        let tx = self.ui_tx.clone();
        let config = self.rockserver.clone();
        if self
            .background
            .spawn(move |_| {
                let client = AccountClient::new(config, OsCredentialStore);
                let result = client.load_account_session();
                let _ = tx.send(super::super::messages::UiMsg::AccountLoaded(result));
            })
            .is_err()
        {
            self.account_load_started = false;
            self.account_refreshing = false;
            self.account_state = AccountUiState::Error {
                kind: AccountErrorKind::Recoverable,
                cached: None,
            };
        }
    }

    fn start_pairing(&mut self, name: String) {
        let fallback_name = name.clone();
        self.pairing_link_copied = false;
        self.account_state = AccountUiState::Starting {
            device_name: name.clone(),
            loading_account: false,
        };
        let tx = self.ui_tx.clone();
        let config = self.rockserver.clone();
        if self
            .background
            .spawn(move |_| {
                let result = AccountClient::new(config, OsCredentialStore).create_pairing(&name);
                let _ = tx.send(super::super::messages::UiMsg::PairingStarted { name, result });
            })
            .is_err()
        {
            self.account_state = AccountUiState::Disconnected {
                device_name: fallback_name,
                message: Some(self.lang.t().account_connection_failed.into()),
            };
        }
    }

    fn cancel_pairing(&mut self) {
        if let Some(cancel) = &self.pairing_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.pairing_cancel = None;
        self.pairing_link_copied = false;
        self.account_state = AccountUiState::Disconnected {
            device_name: default_device_name(),
            message: Some(self.lang.t().account_connection_cancelled.into()),
        };
    }

    pub(in crate::app) fn draw_account_window(&mut self, ctx: &Context) {
        if !self.account_open {
            return;
        }
        self.begin_account_load();
        let mut open = self.account_open;
        egui::Window::new(
            RichText::new(self.lang.t().account_title)
                .size(FS_TITLE)
                .strong(),
        )
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .default_width(430.0)
        .anchor(Align2::RIGHT_TOP, [-16.0, 46.0])
        .show(ctx, |ui| self.draw_account_state(ui, ctx));
        if !self.account_open {
            open = false;
        }
        if self.account_open
            && !open
            && matches!(self.account_state, AccountUiState::Waiting { .. })
        {
            self.cancel_pairing();
        }
        if !open {
            self.account_refreshing = false;
            if matches!(
                self.account_state,
                AccountUiState::Starting {
                    loading_account: true,
                    ..
                }
            ) {
                self.account_load_started = false;
                self.account_state = AccountUiState::Disconnected {
                    device_name: default_device_name(),
                    message: None,
                };
            }
        }
        self.account_open = open;
    }

    fn draw_account_state(&mut self, ui: &mut egui::Ui, ctx: &Context) {
        enum Action {
            StartPairing(String),
            Refresh,
            Logout,
            Cancel,
            OpenDevices,
            Done,
            AskRevoke(String),
            Revoke(String),
        }
        let mut action = None;
        let t = self.lang.t();
        match &mut self.account_state {
            AccountUiState::Disconnected {
                device_name,
                message,
            } => {
                ui.label(t.account_offline_note);
                if let Some(message) = message {
                    ui.label(message.as_str());
                }
                ui.separator();
                ui.label(RichText::new(t.account_connect_title).strong());
                ui.horizontal(|ui| {
                    ui.label(t.account_pc_name);
                    ui.add(
                        egui::TextEdit::singleline(device_name)
                            .desired_width(260.0)
                            .char_limit(128),
                    );
                });
                if ui
                    .add_enabled(
                        !device_name.trim().is_empty(),
                        egui::Button::new(t.account_connect),
                    )
                    .clicked()
                {
                    action = Some(Action::StartPairing(device_name.trim().to_owned()));
                }
            }
            AccountUiState::Starting {
                device_name,
                loading_account,
            } => {
                ui.spinner();
                ui.label(if *loading_account {
                    t.account_checking
                } else {
                    t.account_starting
                });
                if !*loading_account {
                    ui.label(i18n::fmt1(
                        t.account_device,
                        presentation_device_name(std::env::consts::OS, device_name),
                    ));
                }
                ui.label(t.account_offline_note);
            }
            AccountUiState::Waiting { request, status } => {
                ui.label(RichText::new(t.account_waiting_title).strong());
                ui.label(t.account_waiting_step);
                ui.label(status.as_str());
                ui.label(i18n::fmt1(
                    t.account_device,
                    presentation_device_name(&request.device_type, &request.device_display_name),
                ));
                ui.label(i18n::fmt1(
                    t.account_expires_in,
                    countdown(&request.expires_at),
                ));
                ui.label(t.account_waiting_steps);
                ui.label(i18n::fmt1(t.account_phrase, &request.verification_phrase));
                ui.label(i18n::fmt1(t.account_short_code, &request.short_code));
                let link = request.deep_link(self.rockserver.base_url());
                draw_qr(ui, &link);
                if ui.button(t.account_open_link).clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(link.clone()));
                }
                if ui.button(t.account_copy_link).clicked() {
                    ui.ctx().copy_text(link);
                    self.pairing_link_copied = true;
                }
                if self.pairing_link_copied {
                    ui.label(t.account_link_copied);
                }
                if ui.button(t.account_cancel).clicked() {
                    action = Some(Action::Cancel);
                }
            }
            AccountUiState::ConnectedFirstTime { context } => {
                let pc_icon = self.app_icons.pc.id();
                ui.label(RichText::new(t.account_success_title).size(FS_ROW).strong());
                ui.add_space(4.0);
                draw_current_account(ui, context, t, pc_icon);
                self.draw_sync_status(ui, t);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 8.0) * 0.5;
                    if ui
                        .add_sized([w, 32.0], ghost_button(t.account_open_devices))
                        .clicked()
                    {
                        action = Some(Action::OpenDevices);
                    }
                    if ui
                        .add_sized([w, 32.0], ghost_button(t.account_done))
                        .clicked()
                    {
                        action = Some(Action::Done);
                    }
                });
            }
            AccountUiState::Connected { context, banner } => {
                let pc_icon = self.app_icons.pc.id();
                let phone_icon = self.app_icons.phone.id();
                if let Some(banner) = banner {
                    ui.label(RichText::new(banner.as_str()).color(MUTED).size(FS_SMALL));
                    ui.add_space(4.0);
                }
                if let Some(id) = draw_connected(ui, context, t, self.lang, pc_icon, phone_icon) {
                    action = Some(Action::AskRevoke(id));
                }
                self.draw_sync_status(ui, t);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 8.0) * 0.5;
                    let refresh_label: String = if self.account_refreshing {
                        t.account_checking.into()
                    } else {
                        t.account_refresh_short.into()
                    };
                    if ui
                        .add_sized([w, 32.0], ghost_button(&refresh_label))
                        .clicked()
                        && !self.account_refreshing
                    {
                        action = Some(Action::Refresh);
                    }
                    if ui
                        .add_sized(
                            [w, 32.0],
                            egui::Button::new(
                                RichText::new(t.account_logout).color(DANGER).size(FS_BODY),
                            )
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::new(1.0, danger_dim()))
                            .corner_radius(CornerRadius::same(6)),
                        )
                        .clicked()
                    {
                        action = Some(Action::Logout);
                    }
                });
            }
            AccountUiState::Error { kind, cached } => {
                ui.label(match kind {
                    AccountErrorKind::Recoverable => t.account_unavailable,
                    AccountErrorKind::SecureStorage => t.account_storage_unavailable,
                });
                if let Some(context) = cached {
                    ui.label(t.account_devices_unavailable);
                    let pc_icon = self.app_icons.pc.id();
                    draw_current_account(ui, context, t, pc_icon);
                }
            }
        }
        if self.revoke_confirmation.is_some() {
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(
                RichText::new(t.account_confirm_disconnect)
                    .color(FG)
                    .size(FS_BODY)
                    .strong(),
            );
            ui.add_space(4.0);
            let w = (ui.available_width() - 8.0) * 0.5;
            if ui
                .add_sized(
                    [w, 32.0],
                    egui::Button::new(
                        RichText::new(t.account_confirm)
                            .color(Color32::WHITE)
                            .size(FS_BODY),
                    )
                    .fill(Color32::from_rgb(0xdc, 0x26, 0x26))
                    .corner_radius(CornerRadius::same(6)),
                )
                .clicked()
            {
                action = self.revoke_confirmation.clone().map(Action::Revoke);
            }
            if ui
                .add_sized([w, 32.0], ghost_button(t.account_done))
                .clicked()
            {
                self.revoke_confirmation = None;
            }
        }
        match action {
            Some(Action::StartPairing(name)) => self.start_pairing(name),
            Some(Action::Refresh) => self.refresh_account(),
            Some(Action::Logout) => {
                let _ = AccountClient::new(self.rockserver.clone(), OsCredentialStore).logout();
                self.account_state = AccountUiState::Disconnected {
                    device_name: default_device_name(),
                    message: None,
                };
            }
            Some(Action::Cancel) => self.cancel_pairing(),
            Some(Action::OpenDevices) => {
                if let AccountUiState::ConnectedFirstTime { context } = &self.account_state {
                    self.account_state = AccountUiState::Connected {
                        context: context.clone(),
                        banner: None,
                    };
                }
            }
            Some(Action::Done) => self.account_open = false,
            Some(Action::AskRevoke(id)) => self.revoke_confirmation = Some(id),
            Some(Action::Revoke(id)) => {
                self.revoke_confirmation = None;
                if AccountClient::new(self.rockserver.clone(), OsCredentialStore)
                    .revoke_device(&id)
                    .is_ok()
                {
                    self.refresh_account();
                }
            }
            None => {}
        }
    }
}

fn draw_current_account(
    ui: &mut egui::Ui,
    context: &AccountContext,
    t: &i18n::Strings,
    pc_icon: egui::TextureId,
) {
    ui.horizontal(|ui| {
        icon_tile(ui, pc_icon);
        ui.label(
            RichText::new(presentation_device_name(
                &context.profile.device_type,
                &context.profile.device_display_name,
            ))
            .size(FS_ROW)
            .strong(),
        );
        if ui.available_width() > 90.0 {
            status_chip(ui, t.account_connected);
        }
    });
    ui.label(
        RichText::new(i18n::fmt1(
            t.account_success,
            &context.profile.account_display_name,
        ))
        .color(MUTED)
        .size(FS_SMALL),
    );
}

fn draw_connected(
    ui: &mut egui::Ui,
    context: &AccountContext,
    t: &i18n::Strings,
    lang: i18n::Lang,
    pc_icon: egui::TextureId,
    phone_icon: egui::TextureId,
) -> Option<String> {
    let mut revoke = None;

    section_title(ui, t.account_section_this_pc);
    ui.add_space(4.0);
    draw_current_account(ui, context, t, pc_icon);

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);
    section_title(ui, t.account_other_devices);

    let others: Vec<&crate::session::Device> = context
        .devices
        .iter()
        .filter(|device| device.device_id != context.profile.device_id)
        .collect();
    if others.is_empty() {
        ui.label(
            RichText::new(t.account_empty_devices)
                .color(MUTED)
                .size(FS_SMALL),
        );
    }
    for device in others {
        ui.add_space(4.0);
        let icon = if device.device_type.to_ascii_lowercase().contains("mobile") {
            phone_icon
        } else {
            pc_icon
        };
        ui.horizontal(|ui| {
            icon_tile(ui, icon);
            ui.label(
                RichText::new(presentation_device_name(
                    &device.device_type,
                    &device.device_display_name,
                ))
                .size(FS_BODY)
                .strong(),
            );
            if ui.available_width() > 170.0 {
                status_chip(ui, t.account_connected);
            }
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if danger_button(ui, t.account_disconnect, false).clicked() {
                    revoke = Some(device.device_id.clone());
                }
            });
        });
        let mut meta = String::new();
        if let Some(date) = display_date(&device.created_at, lang) {
            meta.push_str(&i18n::fmt1(t.account_connected_at, date));
        }
        if let Some(date) = device
            .last_seen_at
            .as_deref()
            .and_then(|value| display_date(value, lang))
        {
            if !meta.is_empty() {
                meta.push('\n');
            }
            meta.push_str(&i18n::fmt1(t.account_last_seen, date));
        }
        if !meta.is_empty() {
            ui.horizontal(|ui| {
                // Align meta text under the device name, past the 34px tile.
                ui.add_space(42.0);
                ui.label(RichText::new(meta).color(MUTED).size(FS_SMALL));
            });
        }
    }
    revoke
}

fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .size(FS_MICRO)
            .color(MUTED)
            .strong(),
    );
}

/// 34px rounded tile with a tinted device icon texture from `assets/`.
fn icon_tile(ui: &mut egui::Ui, tex: egui::TextureId) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(8), PANEL_2);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0, BORDER),
        StrokeKind::Inside,
    );
    ui.painter().image(
        tex,
        rect.shrink(9.0),
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::from_rgb(0xd5, 0xca, 0xc0),
    );
}

/// Small green "Подключено" pill.
fn status_chip(ui: &mut egui::Ui, text: &str) {
    let font = FontId::proportional(FS_MICRO);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(84.0, 18.0), Sense::hover());
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(9),
        Color32::from_rgba_unmultiplied(61, 220, 132, 26),
    );
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(9),
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(61, 220, 132, 70)),
        StrokeKind::Inside,
    );
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, text, font, GREEN);
}

fn ghost_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).color(FG).size(FS_BODY))
        .fill(PANEL_2)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(6))
}

fn danger_button(ui: &mut egui::Ui, text: &str, filled: bool) -> egui::Response {
    let btn = if filled {
        egui::Button::new(RichText::new(text).color(Color32::WHITE).size(FS_SMALL))
            .fill(Color32::from_rgb(0xdc, 0x26, 0x26))
            .corner_radius(CornerRadius::same(6))
    } else {
        egui::Button::new(RichText::new(text).color(DANGER).size(FS_SMALL))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, danger_dim()))
            .corner_radius(CornerRadius::same(6))
    };
    ui.add(btn)
}

fn default_device_name() -> String {
    super::super::default_pairing_device_name()
}

pub(super) fn presentation_device_name(device_type: &str, value: &str) -> String {
    let product = if device_type.to_ascii_lowercase().contains("mobile") {
        "RockMobile"
    } else {
        "RockCast"
    };
    let raw = value.trim();
    let stripped = raw
        .strip_prefix("RockMobile — ")
        .or_else(|| raw.strip_prefix("RockMobile - "))
        .or_else(|| raw.strip_prefix("RockCast — "))
        .or_else(|| raw.strip_prefix("RockCast - "))
        .unwrap_or(raw);
    format!("{product} — {stripped}")
}

fn countdown(expires_at: &str) -> String {
    let Ok(expires_at) = OffsetDateTime::parse(expires_at, &Rfc3339) else {
        return "—".into();
    };
    let seconds = (expires_at - OffsetDateTime::now_utc())
        .whole_seconds()
        .max(0);
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn display_date(value: &str, lang: i18n::Lang) -> Option<String> {
    let value = OffsetDateTime::parse(value, &Rfc3339).ok()?;
    Some(if lang == i18n::Lang::Ru {
        format!(
            "{:02}.{:02}.{:04}, {:02}:{:02} UTC",
            value.day(),
            u8::from(value.month()),
            value.year(),
            value.hour(),
            value.minute()
        )
    } else {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02} UTC",
            value.year(),
            u8::from(value.month()),
            value.day(),
            value.hour(),
            value.minute()
        )
    })
}

fn qr_layout(width: usize) -> (usize, usize) {
    const QUIET_ZONE: usize = 4;
    let modules = width + QUIET_ZONE * 2;
    (modules, (320 / modules).max(1))
}

fn draw_qr(ui: &mut egui::Ui, value: &str) {
    let Ok(code) =
        qrcode::QrCode::with_error_correction_level(value.as_bytes(), qrcode::EcLevel::M)
    else {
        return;
    };
    let (modules, module_size) = qr_layout(code.width());
    let size = (modules * module_size) as f32;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::WHITE);
    for y in 0..code.width() {
        for x in 0..code.width() {
            if code[(x, y)] == qrcode::Color::Dark {
                let offset = 4 + x;
                let row = 4 + y;
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        rect.min
                            + egui::vec2((offset * module_size) as f32, (row * module_size) as f32),
                        egui::vec2(module_size as f32, module_size as f32),
                    ),
                    0.0,
                    egui::Color32::BLACK,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AccountContext, AccountUiState, presentation_device_name, qr_layout};
    use crate::session::AccountProfile;
    #[derive(Debug, PartialEq, Eq)]
    struct ScreenFlags {
        connect: bool,
        qr: bool,
        expired_error: bool,
    }
    fn flags(state: &AccountUiState) -> ScreenFlags {
        match state {
            AccountUiState::Disconnected { .. } => ScreenFlags {
                connect: true,
                qr: false,
                expired_error: false,
            },
            AccountUiState::Starting { .. }
            | AccountUiState::ConnectedFirstTime { .. }
            | AccountUiState::Connected { .. } => ScreenFlags {
                connect: false,
                qr: false,
                expired_error: false,
            },
            AccountUiState::Waiting { .. } => ScreenFlags {
                connect: false,
                qr: true,
                expired_error: false,
            },
            AccountUiState::Error { .. } => ScreenFlags {
                connect: false,
                qr: false,
                expired_error: true,
            },
        }
    }
    #[test]
    fn device_presentation_uses_product_from_device_type() {
        assert_eq!(
            presentation_device_name("windows", "RockCast — DESKTOP"),
            "RockCast — DESKTOP"
        );
        assert_eq!(
            presentation_device_name("rockmobile_android", "RMX5056"),
            "RockMobile — RMX5056"
        );
        assert_eq!(
            presentation_device_name("rockmobile_android", "RockMobile — RMX5056"),
            "RockMobile — RMX5056"
        );
        assert_eq!(
            presentation_device_name("rockmobile_android", "RockCast — RMX5056"),
            "RockMobile — RMX5056"
        );
        assert_eq!(
            presentation_device_name("windows", "RockCast — Office"),
            "RockCast — Office"
        );
    }
    #[test]
    fn qr_uses_a_four_module_quiet_zone_and_integer_modules() {
        let (modules, module_size) = qr_layout(45);
        assert_eq!(modules, 53);
        assert_eq!(module_size, 6);
        assert!((256..=320).contains(&(modules * module_size)));
    }
    #[test]
    fn account_session_active_only_for_connected_states() {
        use crate::app::{AccountContext, AccountUiState, account_session_active};
        assert!(!account_session_active(&AccountUiState::Disconnected {
            device_name: "PC".into(),
            message: None,
        }));
        assert!(account_session_active(
            &AccountUiState::ConnectedFirstTime {
                context: AccountContext {
                    profile: AccountProfile {
                        device_id: "device".into(),
                        account_display_name: "account".into(),
                        device_display_name: "DESKTOP".into(),
                        device_type: "windows".into(),
                    },
                    devices: Vec::new(),
                },
            }
        ));
    }

    #[test]
    fn connected_and_loading_screens_never_offer_pairing() {
        let loading = AccountUiState::Starting {
            device_name: "DESKTOP".into(),
            loading_account: true,
        };
        let connected = AccountUiState::Connected {
            context: AccountContext {
                profile: AccountProfile {
                    device_id: "device".into(),
                    account_display_name: "account".into(),
                    device_display_name: "DESKTOP".into(),
                    device_type: "windows".into(),
                },
                devices: Vec::new(),
            },
            banner: None,
        };
        assert_eq!(flags(&connected), flags(&loading));
    }
}
