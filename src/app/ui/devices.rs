//! egui panel widgets.

use eframe::egui::{self, RichText, Ui, Vec2};

use super::super::RockCastApp;
use super::super::theme::*;

impl RockCastApp {
    pub(in crate::app) fn draw_device_row(&mut self, ui: &mut Ui) {
        let t = self.lang.t();
        let cast_selected = self
            .selected_device
            .and_then(|i| self.devices.get(i))
            .is_some_and(|d| !d.is_local());

        ui.horizontal(|ui| {
            ui.set_height(28.0);
            ui.label(RichText::new(t.device).color(MUTED));
            ui.add_space(8.0);

            let labels: Vec<String> = self
                .devices
                .iter()
                .map(|d| d.label(self.lang))
                .collect();
            let find_w = 125.0;
            let relay_w = if cast_selected { 110.0 } else { 0.0 };
            let combo_w = (ui.available_width() - find_w - relay_w - 20.0).clamp(160.0, 480.0);

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
                .min_size(Vec2::new(find_w, 26.0))
                .fill(PANEL_2);
            if ui.add_enabled(!self.loading_devices, find).clicked() {
                self.refresh_devices();
            }

            if cast_selected {
                ui.add_space(10.0);
                let mut relay = self.cast_relay;
                let toggle = ui
                    .checkbox(
                        &mut relay,
                        RichText::new(t.cast_relay).color(FG).size(12.5),
                    )
                    .on_hover_text(format!("{}\n{}", t.cast_relay_hint, t.cast_relay_note));
                if toggle.changed() {
                    self.set_cast_relay(relay);
                }
            }
        });
    }
}
