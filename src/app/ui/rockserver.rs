//! egui panel widgets.

use eframe::egui::{self, Align, Color32, Layout, RichText, Ui, Vec2};

use super::super::RockCastApp;
use super::super::theme::*;
use crate::settings::RockServerVoiceMode;

impl RockCastApp {
    pub(in crate::app) fn draw_rockserver_panel(&mut self, ui: &mut Ui) {
        let t = self.lang.t();
        let token_saved = !self.rockserver_bearer_token.trim().is_empty();
        panel(ui, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    let mut enabled = self.rockserver_enabled;
                    if ui.checkbox(&mut enabled, t.rockserver).changed() {
                        self.rockserver_enabled = enabled;
                        self.mark_settings_dirty();
                        self.refresh_stations();
                        if enabled && !token_saved {
                            self.rockserver_setup_open = true;
                        }
                        if !enabled {
                            self.rockserver_setup_open = false;
                        }
                    }
                    if self.rockserver_enabled {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let label = if self.rockserver_setup_open {
                                t.rockserver_hide
                            } else {
                                t.rockserver_configure
                            };
                            let btn = egui::Button::new(RichText::new(label).color(FG).size(12.0))
                                .fill(PANEL_2)
                                .min_size(Vec2::new(72.0, 22.0));
                            if ui.add(btn).clicked() {
                                self.rockserver_setup_open = !self.rockserver_setup_open;
                            }
                        });
                    }
                });

                if !self.rockserver_enabled {
                    ui.label(
                        RichText::new(t.rockserver_autonomous)
                            .color(MUTED)
                            .size(12.0),
                    );
                    return;
                }

                if !self.rockserver_setup_open {
                    let status = if token_saved {
                        RichText::new(t.rockserver_status_ok)
                            .color(Color32::from_rgb(0x7a, 0xc9, 0x6a))
                    } else {
                        RichText::new(t.rockserver_status_need_token).color(ACCENT)
                    };
                    ui.horizontal(|ui| {
                        ui.label(status.size(12.0));
                        let url_hint = self.rockserver_url.trim();
                        if !url_hint.is_empty() {
                            ui.label(
                                RichText::new(format!("· {}", truncate(url_hint, 42)))
                                    .color(MUTED)
                                    .size(12.0),
                            );
                        }
                    });
                    return;
                }

                ui.add_space(4.0);
                ui.label(RichText::new(t.rockserver_url).color(MUTED));
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.rockserver_url)
                            .desired_width(ui.available_width()),
                    )
                    .lost_focus()
                {
                    self.mark_settings_dirty();
                }
                ui.add_space(4.0);
                ui.label(RichText::new(t.rockserver_token).color(MUTED));
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.rockserver_bearer_token)
                            .password(true)
                            .desired_width(ui.available_width()),
                    )
                    .lost_focus()
                {
                    self.mark_settings_dirty();
                }
                ui.add_space(4.0);
                ui.label(RichText::new(t.rockserver_voice_mode).color(MUTED));
                let previous_mode = self.rockserver_voice_mode;
                egui::ComboBox::from_id_salt("rockserver_voice_mode")
                    .selected_text(match self.rockserver_voice_mode {
                        RockServerVoiceMode::BufferedV1 => t.rockserver_voice_buffered,
                        RockServerVoiceMode::StreamingV3 => t.rockserver_voice_streaming,
                    })
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.rockserver_voice_mode,
                            RockServerVoiceMode::BufferedV1,
                            t.rockserver_voice_buffered,
                        );
                        ui.selectable_value(
                            &mut self.rockserver_voice_mode,
                            RockServerVoiceMode::StreamingV3,
                            t.rockserver_voice_streaming,
                        );
                    });
                if self.rockserver_voice_mode != previous_mode {
                    self.mark_settings_dirty();
                }
                ui.add_space(2.0);
                ui.label(
                    RichText::new(t.rockserver_token_hint)
                        .color(MUTED)
                        .size(11.0),
                );
            });
        });
    }
}
