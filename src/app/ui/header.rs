//! Application top header bar using egui-taffy.

use eframe::egui::{
    self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, StrokeKind, Ui, Vec2,
};
use egui_taffy::{
    taffy::{
        self,
        prelude::{auto, length, percent, AlignItems, FlexDirection, JustifyContent, Size},
    },
    tui, TuiBuilderLogic,
};

use super::super::{
    Lang, RockCastApp, account_session_active,
    theme::{self, ACCENT, BORDER, FG, MUTED, PANEL_2},
};

impl RockCastApp {
    pub(in crate::app) fn draw_header(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        let mut switch_lang = None;
        let mut open_account = false;

        let acc_color = if account_session_active(&self.account_state) {
            theme::GREEN
        } else {
            FG
        };
        let acc_label = self.lang.t().account_menu;
        let current_lang = self.lang;

        tui(ui, ui.id().with("app_header"))
            .reserve_available_width()
            .style(taffy::Style {
                flex_direction: FlexDirection::Row,
                justify_content: Some(JustifyContent::SpaceBetween),
                align_items: Some(AlignItems::Center),
                size: Size {
                    width: percent(1.0_f32),
                    height: length(32.0_f32),
                },
                ..Default::default()
            })
            .show(|tui| {
                // Left side: Brand mark, title, desktop badge
                tui.style(taffy::Style {
                    flex_shrink: 0.0,
                    size: Size {
                        width: auto(),
                        height: length(32.0_f32),
                    },
                    ..Default::default()
                })
                .ui(|ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);
                        let (logo_rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                        ui.painter().image(
                            self.app_icons.logo.id(),
                            logo_rect,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        ui.label(
                            RichText::new("RockCast")
                                .size(theme::FS_TITLE)
                                .color(FG)
                                .strong(),
                        );
                        let (badge_rect, _) =
                            ui.allocate_exact_size(Vec2::new(56.0, 18.0), Sense::hover());
                        ui.painter().rect_filled(
                            badge_rect,
                            CornerRadius::same(4),
                            Color32::from_rgba_unmultiplied(229, 96, 32, 35),
                        );
                        ui.painter().rect_stroke(
                            badge_rect,
                            CornerRadius::same(4),
                            egui::Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(229, 96, 32, 90)),
                            StrokeKind::Inside,
                        );
                        ui.painter().text(
                            badge_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "DESKTOP",
                            egui::FontId::proportional(9.5),
                            ACCENT,
                        );
                    });
                });

                // Right side: Account button and Language menu
                tui.style(taffy::Style {
                    flex_shrink: 0.0,
                    size: Size {
                        width: auto(),
                        height: length(32.0_f32),
                    },
                    ..Default::default()
                })
                .ui(|ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);
                        let acc_btn = egui::Button::new(
                            RichText::new(acc_label)
                                .color(acc_color)
                                .size(theme::FS_BODY),
                        )
                        .fill(PANEL_2)
                        .stroke(egui::Stroke::new(1.0_f32, BORDER));
                        if ui.add(acc_btn).clicked() {
                            open_account = true;
                        }

                        ui.menu_button(
                            RichText::new(current_lang.native_name())
                                .color(MUTED)
                                .size(theme::FS_BODY),
                            |ui| {
                                for lang in [Lang::Ru, Lang::En] {
                                    let selected = current_lang == lang;
                                    if ui.selectable_label(selected, lang.native_name()).clicked() {
                                        if current_lang != lang {
                                            switch_lang = Some(lang);
                                        }
                                        ui.close();
                                    }
                                }
                            },
                        );
                    });
                });
            });

        if let Some(lang) = switch_lang {
            self.set_language(ctx, lang);
        }
        if open_account {
            self.account_open = true;
        }

        ui.label(
            RichText::new(self.lang.t().subtitle)
                .size(theme::FS_SMALL)
                .color(MUTED),
        );
        ui.add_space(8.0);
        self.draw_device_row(ui);
        ui.add_space(6.0);
    }
}
