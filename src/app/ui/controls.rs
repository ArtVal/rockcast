//! egui panel widgets.

use std::time::Duration;

use eframe::egui::{self, Align, Color32, CornerRadius, Frame, Layout, RichText, Stroke, Ui, Vec2};

use super::super::RockCastApp;
use super::super::theme::*;

impl RockCastApp {
    pub(in crate::app) fn draw_now_playing(&mut self, ui: &mut Ui) {
        panel(ui, |ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), 60.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.vertical(|ui| {
                        let text_w = (ui.available_width() - 320.0).max(160.0);
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
                        self.draw_eq(ui, Vec2::new(210.0, 46.0));
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
    pub(in crate::app) fn draw_controls(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.set_height(30.0);
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
                        .min_size(Vec2::new(68.0, 28.0))
                        .fill(voice_color)
                        .stroke(Stroke::new(2.0, voice_color.gamma_multiply(0.65)));
                let response = ui
                    .add(voice)
                    .on_hover_text("Удерживайте кнопку и говорите; отпустите для распознавания");
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
                let stop = egui::Button::new(RichText::new("Stop").color(FG))
                    .min_size(Vec2::new(68.0, 28.0))
                    .fill(PANEL_2);
                if ui.add_enabled(!self.shutting_down, stop).clicked() {
                    log::info!("UI Stop clicked");
                    self.stop();
                }
                ui.add_space(6.0);
                let play = egui::Button::new(RichText::new("Play").strong().color(Color32::WHITE))
                    .min_size(Vec2::new(82.0, 28.0))
                    .fill(ACCENT);
                if ui.add_enabled(self.can_start_play(), play).clicked() {
                    log::info!("UI Play clicked");
                    self.play();
                }
            });
        });
    }
    pub(in crate::app) fn draw_status(&self, ui: &mut Ui) {
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
