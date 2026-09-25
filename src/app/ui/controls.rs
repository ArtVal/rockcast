//! egui player controls and now playing panel.

use eframe::egui::{
    Align, Color32, CornerRadius, FontId, Frame, Layout, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, Ui, Vec2,
};

use super::super::RockCastApp;
use super::super::theme::*;

impl RockCastApp {
    /// Renders the unified rock-styled player deck:
    /// station info (left) · transport on the panel axis (center) ·
    /// spectrum centered in the right half · status footer with volume.
    pub(in crate::app) fn draw_player_deck(&mut self, ui: &mut Ui) {
        let t = self.lang.t();
        let (initial, icon_texture_id, fallback_st_name) = {
            let current_st = self
                .selected_station
                .and_then(|i| self.stations.get(i))
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

        Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    let full_w = ui.available_width();
                    // One 64px row on a single vertical center line: the logo
                    // and transport button are 48px inside it, the spectrum
                    // spans the full row height.
                    let row_h = 64.0;
                    let (deck_rect, _) =
                        ui.allocate_exact_size(Vec2::new(full_w, row_h), Sense::hover());
                    let cy = deck_rect.center().y;

                    // --- SECTION 1: Station Info (Left) ---
                    let tp_left_x = deck_rect.center().x - 24.0;
                    let left_w = (tp_left_x - 24.0 - deck_rect.left()).max(180.0);
                    let left_rect = Rect::from_min_size(deck_rect.min, Vec2::new(left_w, row_h));
                    ui.scope_builder(egui::UiBuilder::new().max_rect(left_rect), |ui| {
                        // Station art / monogram — 48px, centered like every
                        // other element in the row.
                        let logo_rect = Rect::from_center_size(
                            Pos2::new(left_rect.left() + 24.0, cy),
                            Vec2::splat(48.0),
                        );
                        if let Some(tex_id) = icon_texture_id {
                            ui.painter().image(
                                tex_id,
                                logo_rect,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        } else {
                            let monogram_bg = station_color(&fallback_st_name);
                            ui.painter()
                                .rect_filled(logo_rect, CornerRadius::same(8), monogram_bg);
                            ui.painter().rect_stroke(
                                logo_rect,
                                CornerRadius::same(8),
                                Stroke::new(1.5, ACCENT),
                                StrokeKind::Inside,
                            );
                            ui.painter().text(
                                logo_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                initial,
                                FontId::proportional(FS_ROW),
                                Color32::from_rgb(0xfa, 0xee, 0xe4),
                            );
                        }

                        // Two text lines, precisely centered on the row axis.
                        let st_name = if self.station_now.is_empty() {
                            fallback_st_name.as_str()
                        } else {
                            &self.station_now
                        };
                        let track_display = if self.track.is_empty() {
                            "Прямой эфир"
                        } else {
                            &self.track
                        };
                        let line2 = if self.playing {
                            track_display
                        } else {
                            t.stopped
                        };
                        let text_x = left_rect.left() + 60.0;
                        let text_w = (left_rect.right() - text_x).max(80.0);
                        let name_font = FontId::proportional(FS_ROW);
                        let track_font = FontId::proportional(FS_BODY);
                        let name_h = ui
                            .painter()
                            .layout_no_wrap(st_name.to_owned(), name_font.clone(), FG)
                            .size()
                            .y;
                        let track_h = ui
                            .painter()
                            .layout_no_wrap(line2.to_owned(), track_font.clone(), FG)
                            .size()
                            .y;
                        let gap = 4.0;
                        let pad = ((row_h - name_h - gap - track_h) * 0.5).max(0.0);

                        let text_rect = Rect::from_min_size(
                            Pos2::new(text_x, left_rect.min.y),
                            Vec2::new(text_w, row_h),
                        );
                        let text_resp =
                            ui.interact(text_rect, ui.id().with("deck_text"), Sense::hover());
                        let name_y = left_rect.min.y + pad + name_h * 0.5;
                        let track_y = name_y + name_h * 0.5 + gap + track_h * 0.5;
                        let now = ui.input(|i| i.time) as f32;
                        // Long names/titles scroll marquee-style inside the
                        // text zone instead of bleeding into the transport
                        // button and the spectrum.
                        let text_painter = ui.painter_at(text_rect);
                        let name_scrolls = draw_marquee_line(
                            &text_painter,
                            text_rect,
                            name_y,
                            st_name,
                            name_font,
                            FG,
                            now,
                        );
                        let track_clip_left = text_x + if self.playing { 12.0 } else { 0.0 };
                        let track_clip = Rect::from_min_max(
                            Pos2::new(track_clip_left, text_rect.top()),
                            Pos2::new(text_rect.right(), text_rect.bottom()),
                        );
                        let track_color = if self.playing {
                            ACCENT
                        } else {
                            MUTED
                        };
                        let track_scrolls = draw_marquee_line(
                            &ui.painter_at(track_clip),
                            track_clip,
                            track_y,
                            line2,
                            track_font,
                            track_color,
                            now,
                        );
                        if self.playing {
                            ui.painter().circle_filled(
                                Pos2::new(text_x + 3.5, track_y),
                                3.5,
                                Color32::from_rgb(0x34, 0xd3, 0x99),
                            );
                        }
                        if name_scrolls || track_scrolls {
                            // Keep the marquee animating even without playback.
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_millis(50));
                        }
                        let _ = text_resp.on_hover_text(format!("{st_name}\n{}", t.track_hint));
                    });

                    // --- SECTION 2: Transport on the panel axis ---
                    {
                        let tp_rect = Rect::from_center_size(
                            Pos2::new(deck_rect.center().x, cy),
                            Vec2::splat(48.0),
                        );
                        let tp_resp =
                            ui.interact(tp_rect, ui.id().with("deck_transport"), Sense::click());
                        let can_play = self.can_start_play();
                        let fill = if self.playing || (can_play && tp_resp.hovered()) {
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
                        let (tp_icon, tp_fg) = if self.playing {
                            ("⏸", Color32::WHITE)
                        } else if can_play {
                            ("▶", Color32::WHITE)
                        } else {
                            ("▶", MUTED)
                        };
                        let x_off = if self.playing { 0.0 } else { 1.5 };
                        ui.painter().text(
                            Pos2::new(tp_rect.center().x + x_off, tp_rect.center().y),
                            egui::Align2::CENTER_CENTER,
                            tp_icon,
                            FontId::proportional(19.0),
                            tp_fg,
                        );
                        if tp_resp.clicked() && !self.shutting_down {
                            if self.playing {
                                log::info!("UI deck transport: stop");
                                self.stop();
                            } else if can_play {
                                log::info!("UI deck transport: play");
                                self.play();
                            }
                        }
                    }

                    // --- SECTION 3: Spectrum centered in the right half ---
                    {
                        let half_center_x = (deck_rect.center().x + deck_rect.right()) * 0.5;
                        let space_left = deck_rect.center().x + 24.0 + 16.0;
                        let space_right = deck_rect.right() - 8.0;
                        let eq_w = ((space_right - space_left) * 0.85).clamp(0.0, 280.0);
                        if eq_w >= 100.0 {
                            let eq_area = Rect::from_center_size(
                                Pos2::new(half_center_x, cy),
                                Vec2::new(eq_w, row_h),
                            );
                            ui.scope_builder(egui::UiBuilder::new().max_rect(eq_area), |ui| {
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    self.draw_eq(ui, Vec2::new(eq_w, row_h));
                                });
                            });
                        }
                    }

                    // --- Footer: status line + volume (left of RockServer) ---
                    ui.add_space(6.0);
                    let sep_y = ui.cursor().top();
                    ui.painter()
                        .hline(ui.max_rect().x_range(), sep_y, Stroke::new(1.0, BORDER));
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        let (dot_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                        let status_dot_color = if self.playing {
                            Color32::from_rgb(0x34, 0xd3, 0x99)
                        } else {
                            MUTED
                        };
                        ui.painter()
                            .circle_filled(dot_rect.center(), 3.5, status_dot_color);
                        ui.add_space(2.0);

                        let srv_label = if self.rockserver.base_url().contains("localhost")
                            || self.rockserver.base_url().contains("127.0.0.1")
                        {
                            "RockServer Local"
                        } else {
                            "RockServer Cloud"
                        };
                        // Reserve fixed space for the right-side group so the
                        // status label can never push it off the panel.
                        let right_side_w = 300.0;
                        let status_w = (ui.available_width() - right_side_w).max(120.0);
                        let status = truncate(&self.status, 90);
                        ui.allocate_ui_with_layout(
                            Vec2::new(status_w, 16.0),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(status).color(MUTED).size(FS_SMALL),
                                    )
                                    .truncate(),
                                );
                            },
                        );

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(srv_label).color(MUTED).size(FS_MICRO));
                            ui.add_space(10.0);

                            ui.label(
                                RichText::new(format!("{:>3}%", self.volume))
                                    .color(MUTED)
                                    .monospace()
                                    .size(FS_SMALL),
                            );
                            ui.add_space(6.0);
                            self.draw_thin_volume_slider(ui, 130.0);
                            ui.add_space(6.0);
                            self.draw_mute_button(ui);
                        });
                    });
                });
            });
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
    let phase = if period > 0.0 { time_secs % period } else { 0.0 };
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
