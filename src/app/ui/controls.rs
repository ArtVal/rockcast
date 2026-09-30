//! egui player controls and now playing panel.

use eframe::egui::{
    Color32, CornerRadius, FontId, Frame, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, Ui, Vec2,
};
use egui_taffy::{
    taffy::{
        self,
        prelude::{auto, length, percent, AlignItems, AlignSelf, FlexDirection, JustifyContent, Size},
    },
    tui, TuiBuilderLogic,
};

use super::super::RockCastApp;
use super::super::theme::*;

impl RockCastApp {
    /// Renders the unified rock-styled player deck using egui-taffy:
    /// station info (left) · transport on the panel axis (center) ·
    /// spectrum centered in the right half · status footer with volume.
    pub(in crate::app) fn draw_player_deck(&mut self, ui: &mut Ui) {
        let t = self.lang.t();
        let (initial, icon_texture_id, fallback_st_name) = {
            let current_st = self
                .resolve_playing_station_index()
                .and_then(|i| self.stations.get(i))
                .or_else(|| {
                    self.playback_station_id
                        .as_deref()
                        .and_then(|id| self.stations.iter().find(|s| s.id == id))
                })
                .or_else(|| self.selected_station.and_then(|i| self.stations.get(i)))
                .or_else(|| self.stations.iter().find(|s| s.name == self.station_now));
            let init = current_st.map_or_else(
                || "RC".to_string(),
                |st| {
                    let chars: Vec<char> =
                        st.name.chars().filter(|c| c.is_alphanumeric()).collect();
                    if chars.len() >= 2 {
                        format!("{}{}", chars[0], chars[1]).to_uppercase()
                    } else if let Some(c) = chars.first() {
                        c.to_uppercase().to_string()
                    } else {
                        "RC".to_string()
                    }
                },
            );
            let tex = current_st.and_then(|st| {
                let source =
                    crate::station_icons::source_url(st, Some(self.rockserver.base_url()))?;
                let request_key = crate::station_icons::request_key(st, &source);
                self.station_icons.get(&request_key).map(|t| t.id())
            });
            let name = current_st.map_or_else(|| "RockCast Radio".to_string(), |s| s.name.clone());
            (init, tex, name)
        };

        let playing = self.playing;
        let can_play = self.can_start_play();
        let playing_idx = self.resolve_playing_station_index();
        let st_name = if self.station_now.is_empty() {
            fallback_st_name.clone()
        } else {
            self.station_now.clone()
        };
        let track_display = if self.track.is_empty() {
            "Прямой эфир".to_string()
        } else {
            self.track.clone()
        };
        let line2 = if playing {
            track_display
        } else {
            t.stopped.to_string()
        };

        let mut scroll_to_playing = false;
        let mut toggle_transport = false;

        Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                let mut needs_marquee_repaint = false;
                let now = ui.input(|i| i.time) as f32;

                tui(ui, ui.id().with("player_deck"))
                    .reserve_available_width()
                    .style(taffy::Style {
                        flex_direction: FlexDirection::Column,
                        align_items: Some(AlignItems::Stretch),
                        size: Size {
                            width: percent(1.0),
                            height: auto(),
                        },
                        gap: length(6.0),
                        ..Default::default()
                    })
                    .show(|tui| {
                        // --- ROW 1: Controls (Station Info, Transport, Spectrum) ---
                        tui.style(taffy::Style {
                            flex_direction: FlexDirection::Row,
                            align_items: Some(AlignItems::Center),
                            justify_content: Some(JustifyContent::SpaceBetween),
                            size: Size {
                                width: percent(1.0),
                                height: length(64.0),
                            },
                            ..Default::default()
                        })
                        .add(|tui| {
                            // Section 1: Station Info (Left)
                            tui.style(taffy::Style {
                                flex_direction: FlexDirection::Row,
                                align_items: Some(AlignItems::Center),
                                flex_grow: 1.0,
                                flex_shrink: 1.0,
                                flex_basis: length(0.0),
                                min_size: Size {
                                    width: length(120.0),
                                    height: length(64.0),
                                },
                                gap: length(12.0),
                                ..Default::default()
                            })
                            .add(|tui| {
                                // Station Art / Monogram
                                tui.style(taffy::Style {
                                    size: Size {
                                        width: length(48.0),
                                        height: length(48.0),
                                    },
                                    flex_shrink: 0.0,
                                    ..Default::default()
                                })
                                .ui(|ui| {
                                    let (logo_rect, logo_resp) =
                                        ui.allocate_exact_size(Vec2::splat(48.0), Sense::click());
                                    if let Some(tex_id) = icon_texture_id {
                                        ui.painter().image(
                                            tex_id,
                                            logo_rect,
                                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                            Color32::WHITE,
                                        );
                                    } else {
                                        let monogram_bg = station_color(&fallback_st_name);
                                        ui.painter().rect_filled(
                                            logo_rect,
                                            CornerRadius::same(8),
                                            monogram_bg,
                                        );
                                        ui.painter().rect_stroke(
                                            logo_rect,
                                            CornerRadius::same(8),
                                            Stroke::new(1.5, ACCENT),
                                            StrokeKind::Inside,
                                        );
                                        ui.painter().text(
                                            logo_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            &initial,
                                            FontId::proportional(FS_ROW),
                                            Color32::from_rgb(0xfa, 0xee, 0xe4),
                                        );
                                    }
                                    if logo_resp.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    if logo_resp.clicked() {
                                        scroll_to_playing = true;
                                    }
                                    let _ = logo_resp
                                        .on_hover_text("Показать текущую станцию в списке");
                                });

                                // Station Marquee Text
                                tui.style(taffy::Style {
                                    flex_grow: 1.0,
                                    flex_shrink: 1.0,
                                    size: Size {
                                        width: percent(1.0),
                                        height: length(64.0),
                                    },
                                    min_size: Size {
                                        width: length(60.0),
                                        height: length(44.0),
                                    },
                                    ..Default::default()
                                })
                                .ui(|ui| {
                                    let full_rect = ui.max_rect();
                                    let text_resp = ui.interact(
                                        full_rect,
                                        ui.id().with("deck_text"),
                                        Sense::click(),
                                    );
                                    let hovered = text_resp.hovered();
                                    if hovered {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    if text_resp.clicked() {
                                        scroll_to_playing = true;
                                    }
                                    let _ = text_resp
                                        .on_hover_text(format!("{st_name}\n{}", t.track_hint));

                                    let name_font = FontId::proportional(FS_ROW);
                                    let track_font = FontId::proportional(FS_BODY);
                                    let name_h = ui
                                        .painter()
                                        .layout_no_wrap(st_name.clone(), name_font.clone(), FG)
                                        .size()
                                        .y;
                                    let track_h = ui
                                        .painter()
                                        .layout_no_wrap(line2.clone(), track_font.clone(), FG)
                                        .size()
                                        .y;
                                    let gap = 4.0;
                                    let pad = ((full_rect.height() - name_h - gap - track_h) * 0.5)
                                        .max(0.0);
                                    let name_y = full_rect.min.y + pad + name_h * 0.5;
                                    let track_y = name_y + name_h * 0.5 + gap + track_h * 0.5;

                                    let name_color = if hovered { Color32::WHITE } else { FG };
                                    let text_painter = ui.painter_at(full_rect);
                                    let name_scrolls = draw_marquee_line(
                                        &text_painter,
                                        full_rect,
                                        name_y,
                                        &st_name,
                                        name_font,
                                        name_color,
                                        now,
                                    );

                                    let track_clip_left =
                                        full_rect.left() + if playing { 12.0 } else { 0.0 };
                                    let track_clip = Rect::from_min_max(
                                        Pos2::new(track_clip_left, full_rect.top()),
                                        Pos2::new(full_rect.right(), full_rect.bottom()),
                                    );
                                    let track_color = if playing { ACCENT } else { MUTED };
                                    let track_scrolls = draw_marquee_line(
                                        &ui.painter_at(track_clip),
                                        track_clip,
                                        track_y,
                                        &line2,
                                        track_font,
                                        track_color,
                                        now,
                                    );

                                    if playing {
                                        ui.painter().circle_filled(
                                            Pos2::new(full_rect.left() + 3.5, track_y),
                                            3.5,
                                            Color32::from_rgb(0x34, 0xd3, 0x99),
                                        );
                                    }

                                    if name_scrolls || track_scrolls {
                                        needs_marquee_repaint = true;
                                    }
                                });
                            });

                            // Section 2: Transport on panel axis (Center)
                            tui.style(taffy::Style {
                                size: Size {
                                    width: length(48.0),
                                    height: length(48.0),
                                },
                                flex_shrink: 0.0,
                                align_self: Some(AlignSelf::Center),
                                ..Default::default()
                            })
                            .ui(|ui| {
                                let (tp_rect, tp_resp) =
                                    ui.allocate_exact_size(Vec2::splat(48.0), Sense::click());
                                let fill = if playing || (can_play && tp_resp.hovered()) {
                                    if tp_resp.hovered() {
                                        Color32::from_rgb(0xf0, 0x70, 0x30)
                                    } else {
                                        ACCENT
                                    }
                                } else {
                                    PANEL_2
                                };
                                ui.painter().circle_filled(tp_rect.center(), 24.0, fill);
                                ui.painter().circle_stroke(
                                    tp_rect.center(),
                                    24.0,
                                    Stroke::new(1.0, ACCENT),
                                );
                                let (tp_icon, tp_fg) = if playing {
                                    ("⏸", Color32::WHITE)
                                } else if can_play {
                                    ("▶", Color32::WHITE)
                                } else {
                                    ("▶", MUTED)
                                };
                                let x_off = if playing { 0.0 } else { 1.5 };
                                ui.painter().text(
                                    Pos2::new(tp_rect.center().x + x_off, tp_rect.center().y),
                                    egui::Align2::CENTER_CENTER,
                                    tp_icon,
                                    FontId::proportional(19.0),
                                    tp_fg,
                                );
                                if tp_resp.clicked() {
                                    toggle_transport = true;
                                }
                            });

                            // Section 3: Spectrum centered in right half via Taffy
                            tui.style(taffy::Style {
                                flex_direction: FlexDirection::Row,
                                justify_content: Some(JustifyContent::Center),
                                align_items: Some(AlignItems::Center),
                                flex_grow: 1.0,
                                flex_shrink: 1.0,
                                flex_basis: length(0.0),
                                min_size: Size {
                                    width: length(120.0),
                                    height: length(64.0),
                                },
                                ..Default::default()
                            })
                            .add(|tui| {
                                tui.style(taffy::Style {
                                    size: Size {
                                        width: length(240.0),
                                        height: length(64.0),
                                    },
                                    max_size: Size {
                                        width: percent(0.9),
                                        height: length(64.0),
                                    },
                                    ..Default::default()
                                })
                                .ui(|ui| {
                                    let size = ui.available_size();
                                    self.draw_eq(ui, size);
                                });
                            });
                        });

                        // Separator line
                        tui.style(taffy::Style {
                            size: Size {
                                width: percent(1.0),
                                height: length(1.0),
                            },
                            ..Default::default()
                        })
                        .ui(|ui| {
                            let rect = ui.max_rect();
                            ui.painter().hline(
                                rect.x_range(),
                                rect.center().y,
                                Stroke::new(1.0, BORDER),
                            );
                        });

                        // Footer: status line + volume
                        tui.style(taffy::Style {
                            flex_direction: FlexDirection::Row,
                            align_items: Some(AlignItems::Center),
                            justify_content: Some(JustifyContent::SpaceBetween),
                            size: Size {
                                width: percent(1.0),
                                height: length(20.0),
                            },
                            ..Default::default()
                        })
                        .add(|tui| {
                            // Left: Status dot and message
                            tui.style(taffy::Style {
                                flex_direction: FlexDirection::Row,
                                align_items: Some(AlignItems::Center),
                                flex_grow: 1.0,
                                flex_shrink: 1.0,
                                gap: length(6.0),
                                ..Default::default()
                            })
                            .ui(|ui| {
                                ui.horizontal_centered(|ui| {
                                    let (dot_rect, _) =
                                        ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                                    let status_dot_color = if playing {
                                        Color32::from_rgb(0x34, 0xd3, 0x99)
                                    } else {
                                        MUTED
                                    };
                                    ui.painter().circle_filled(
                                        dot_rect.center(),
                                        3.5,
                                        status_dot_color,
                                    );
                                    let status = truncate(&self.status, 90);
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(status).color(MUTED).size(FS_SMALL),
                                        )
                                        .truncate(),
                                    );
                                });
                            });

                            // Right: Volume and RockServer label
                            tui.style(taffy::Style {
                                flex_shrink: 0.0,
                                size: Size {
                                    width: auto(),
                                    height: length(20.0),
                                },
                                ..Default::default()
                            })
                            .ui(|ui| {
                                ui.horizontal_centered(|ui| {
                                    self.draw_mute_button(ui);
                                    ui.add_space(4.0);
                                    self.draw_thin_volume_slider(ui, 130.0);
                                    ui.add_space(6.0);
                                    ui.label(
                                        RichText::new(format!("{:>3}%", self.volume))
                                            .color(MUTED)
                                            .monospace()
                                            .size(FS_SMALL),
                                    );
                                    ui.add_space(8.0);
                                    let srv_label = if self.rockserver.base_url().contains("localhost")
                                        || self.rockserver.base_url().contains("127.0.0.1")
                                    {
                                        "RockServer Local"
                                    } else {
                                        "RockServer Cloud"
                                    };
                                    ui.label(RichText::new(srv_label).color(MUTED).size(FS_MICRO));
                                });
                            });
                        });
                    });

                if needs_marquee_repaint {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(50));
                }
            });

        if scroll_to_playing {
            if let Some(idx) = playing_idx {
                self.scroll_to_station = Some(idx);
                ui.ctx().request_repaint();
            }
        }
        if toggle_transport && !self.shutting_down {
            if self.playing {
                log::info!("UI deck transport: stop");
                self.stop();
            } else if self.can_start_play() {
                log::info!("UI deck transport: play");
                self.play();
            }
        }
    }

    /// Draws the embedded speaker icon with 1-click mute/unmute.
    fn draw_mute_button(&mut self, ui: &mut Ui) {
        let (spk_rect, spk_resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
        let spk_tex = if self.volume == 0 {
            self.app_icons.speaker_mute.id()
        } else {
            self.app_icons.speaker.id()
        };
        let tint = if spk_resp.hovered() {
            Color32::WHITE
        } else {
            Color32::from_rgb(0xd5, 0xca, 0xc0)
        };
        ui.painter().image(
            spk_tex,
            spk_rect.shrink(1.0),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            tint,
        );
        if spk_resp.clicked() {
            if self.volume > 0 {
                self.volume = 0;
            } else {
                self.volume = 40;
            }
            self.queue_volume();
            self.mark_settings_dirty();
        }
    }

    /// Thin minimal volume slider (3px track, small thumb) for the deck footer.
    fn draw_thin_volume_slider(&mut self, ui: &mut Ui, width: f32) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 14.0), Sense::click_and_drag());
        let frac = f32::from(self.volume) / 100.0;
        let track_y = rect.center().y;
        let track = Rect::from_min_max(
            Pos2::new(rect.left(), track_y - 1.5),
            Pos2::new(rect.right(), track_y + 1.5),
        );
        ui.painter()
            .rect_filled(track, CornerRadius::same(1), BAR_DIM);
        let fill_right = rect.left() + (rect.width() * frac).max(2.0);
        ui.painter().rect_filled(
            Rect::from_min_max(
                Pos2::new(rect.left(), track_y - 1.5),
                Pos2::new(fill_right, track_y + 1.5),
            ),
            CornerRadius::same(1),
            ACCENT,
        );
        let thumb_x = rect.left() + rect.width() * frac;
        let thumb_color = if resp.hovered() || resp.dragged() {
            Color32::WHITE
        } else {
            Color32::from_rgb(0xe8, 0xe0, 0xd8)
        };
        ui.painter()
            .circle_filled(Pos2::new(thumb_x, track_y), 5.0, thumb_color);

        if (resp.dragged() || resp.clicked())
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let frac = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let new_volume = (frac * 100.0).round() as u8;
            if new_volume != self.volume {
                self.volume = new_volume;
                self.queue_volume();
                self.mark_settings_dirty();
            }
        }
    }
}

/// Draws one deck text line clipped to `clip`. Static when it fits; when it
/// overflows, scrolls marquee-style: pause at the start, scroll left until the
/// tail is visible, pause, repeat. Returns `true` while scrolling (so the
/// caller keeps repainting).
fn draw_marquee_line(
    painter: &egui::Painter,
    clip: Rect,
    y_center: f32,
    text: &str,
    font: FontId,
    color: Color32,
    time_secs: f32,
) -> bool {
    let galley = painter.layout_no_wrap(text.to_owned(), font, color);
    let galley_w = galley.size().x;
    if galley_w <= clip.width() {
        painter.galley(
            Pos2::new(clip.left(), y_center - galley.size().y * 0.5),
            galley,
            color,
        );
        return false;
    }

    const SPEED: f32 = 30.0; // px per second
    const PAUSE: f32 = 1.6; // seconds at each end
    let distance = galley_w - clip.width();
    let scroll_time = distance / SPEED;
    let period = scroll_time + PAUSE * 2.0;
    let phase = if period > 0.0 {
        time_secs % period
    } else {
        0.0
    };
    let offset = if phase < PAUSE {
        0.0
    } else if phase < PAUSE + scroll_time {
        (phase - PAUSE) * SPEED
    } else {
        distance
    };
    painter.galley(
        Pos2::new(clip.left() - offset, y_center - galley.size().y * 0.5),
        galley,
        color,
    );
    true
}
