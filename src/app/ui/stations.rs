//! egui panel widgets.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Frame, Layout, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, Ui, Vec2,
};

use super::super::RockCastApp;
use super::super::theme::*;

impl RockCastApp {
    pub(in crate::app) fn draw_station_list(&mut self, ui: &mut Ui, list_h: f32) {
        let t = self.lang.t();
        let mut search_requested = false;
        let mut return_home = false;
        ui.horizontal(|ui| {
            let has_text = !self.station_search.trim().is_empty();
            let is_recording = self.voice_recording.is_some();
            let is_busy = self.voice_busy;

            let catalog_btn_w = 125.0;
            // The search frame adds 2x10px horizontal margin around its content;
            // account for it so the catalog button stays inside the window.
            let frame_margin_x = 20.0;
            let search_box_w =
                (ui.available_width() - catalog_btn_w - 8.0 - frame_margin_x).max(180.0);

            // Cohesive search input container
            Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(10, 4))
                .show(ui, |ui| {
                    ui.set_width(search_box_w);
                    ui.horizontal(|ui| {
                        let (icon_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
                        ui.painter().image(
                            self.app_icons.search.id(),
                            icon_rect,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            MUTED,
                        );
                        let clear_btn_w = if has_text { 24.0 } else { 0.0 };
                        let voice_btn_w = 84.0;
                        let edit_w = (search_box_w - voice_btn_w - clear_btn_w - 28.0).max(80.0);

                        let response = ui.add_sized(
                            [edit_w, 24.0],
                            egui::TextEdit::singleline(&mut self.station_search)
                                .hint_text("Поиск станции, группы или стиля…")
                                .frame(false),
                        );
                        let enter = response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));

                        if has_text {
                            // Icon-asset clear button (the "✕" glyph is not in
                            // the embedded font and renders as a tofu box).
                            let (clr_rect, clr_resp) =
                                ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
                            let tint = if clr_resp.hovered() {
                                Color32::WHITE
                            } else {
                                MUTED
                            };
                            ui.painter().image(
                                self.app_icons.clear.id(),
                                clr_rect.shrink(5.0),
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                tint,
                            );
                            if clr_resp.clicked() {
                                self.station_search.clear();
                                search_requested = true;
                            }
                            let _ = clr_resp.on_hover_text("Очистить поиск / Clear search");
                        }

                        if enter && has_text {
                            search_requested = true;
                        }

                        // Embedded action button, flush with the input's right
                        // edge: voice input while the field is empty, submit
                        // («Найти», no mic) once the user typed something.
                        let (voice_caption, voice_fill, voice_text_color) = if is_recording {
                            ("Стоп", Color32::from_rgb(0xdc, 0x26, 0x26), Color32::WHITE)
                        } else if is_busy {
                            ("Распознаю…", PANEL_2, MUTED)
                        } else if has_text {
                            ("Найти", ACCENT, Color32::WHITE)
                        } else {
                            ("Голос", PANEL_2, ACCENT)
                        };

                        let (voice_rect, voice_resp) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width().max(60.0), 24.0),
                            if !is_busy || is_recording {
                                Sense::click()
                            } else {
                                Sense::hover()
                            },
                        );
                        let hovered_active = voice_resp.hovered() && (!is_busy || is_recording);
                        let bg = if is_recording {
                            if hovered_active {
                                Color32::from_rgb(0xef, 0x44, 0x44)
                            } else {
                                voice_fill
                            }
                        } else if hovered_active {
                            Color32::from_rgb(0xf0, 0x70, 0x30)
                        } else {
                            voice_fill
                        };
                        ui.painter()
                            .rect_filled(voice_rect, CornerRadius::same(6), bg);
                        if !has_text || is_recording || is_busy {
                            ui.painter().rect_stroke(
                                voice_rect,
                                CornerRadius::same(6),
                                Stroke::new(1.0, BORDER),
                                StrokeKind::Inside,
                            );
                        }

                        // The mic icon marks voice mode only (empty field);
                        // «Найти» is text-only so the two never mix.
                        if !is_recording && !is_busy && !has_text {
                            let mic_rect = Rect::from_center_size(
                                Pos2::new(voice_rect.left() + 16.0, voice_rect.center().y),
                                Vec2::splat(15.0),
                            );
                            ui.painter().image(
                                self.app_icons.mic.id(),
                                mic_rect,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                voice_text_color,
                            );
                            ui.painter().text(
                                Pos2::new(voice_rect.left() + 28.0, voice_rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                voice_caption,
                                FontId::proportional(FS_BODY),
                                voice_text_color,
                            );
                        } else {
                            ui.painter().text(
                                voice_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                voice_caption,
                                FontId::proportional(FS_BODY),
                                voice_text_color,
                            );
                        }

                        let clicked = voice_resp.clicked();
                        if clicked {
                            if is_recording {
                                self.stop_voice_recording();
                            } else if has_text {
                                // «Найти»: submit the typed query.
                                search_requested = true;
                            } else if !is_busy {
                                self.start_voice();
                            }
                        } else if enter && !has_text && !is_busy && !is_recording {
                            // Enter on an empty field starts voice input.
                            self.start_voice();
                        }
                    });
                });

            // Quick catalog refresh button
            let cat_label = format!("Каталог ({})", self.stations.len());
            let cat_btn = egui::Button::new(RichText::new(cat_label).color(FG).size(12.0))
                .min_size(Vec2::new(catalog_btn_w, 32.0))
                .corner_radius(CornerRadius::same(8))
                .stroke(Stroke::new(1.0, BORDER))
                .fill(PANEL);
            if ui.add(cat_btn).clicked() {
                return_home = true;
            }
        });

        if self.voice_recording.is_some() || self.voice_busy {
            ui.add_space(2.0);
            Frame::new()
                .fill(Color32::from_rgba_unmultiplied(229, 96, 32, 26))
                .stroke(Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(229, 96, 32, 75),
                ))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (dot_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                        ui.painter().circle_filled(dot_rect.center(), 3.5, ACCENT);
                        let text = if self.voice_recording.is_some() {
                            "Слушаю вас… Назовите станцию или жанр (запись завершится сама)"
                        } else {
                            "Распознаю голос и ищу подходящую станцию в каталоге…"
                        };
                        ui.label(RichText::new(text).color(FG).size(12.0).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .button(RichText::new("✕ Отмена").size(11.0).color(MUTED))
                                .clicked()
                            {
                                self.cancel_voice();
                            }
                        });
                    });
                });
        }

        ui.horizontal_wrapped(|ui| {
            let fav_count = self.personal_data.as_ref().map_or(0, |store| {
                store.favourites().len()
                    + store
                        .profile()
                        .unresolved_references
                        .iter()
                        .filter(|entry| entry.source_kind == "favourite")
                        .count()
            });
            let hist_count = self.personal_data.as_ref().map_or(0, |store| {
                store.history().len()
                    + store
                        .profile()
                        .unresolved_references
                        .iter()
                        .filter(|entry| entry.source_kind == "history")
                        .count()
            });

            // Filter chips: All, Favourites, History, Genres
            let all_selected = self.filter_mode == super::super::StationFilterMode::All
                && self.selected_genre.is_none();
            let all_btn = egui::Button::new(
                RichText::new(format!("Все ({})", self.stations.len()))
                    .color(if all_selected { Color32::WHITE } else { MUTED })
                    .strong(),
            )
            .corner_radius(CornerRadius::same(12))
            .stroke(if all_selected {
                Stroke::NONE
            } else {
                Stroke::new(1.0, BORDER)
            })
            .fill(if all_selected { ACCENT } else { PANEL });
            if ui.add(all_btn).clicked() {
                self.filter_mode = super::super::StationFilterMode::All;
                self.selected_genre = None;
                search_requested = true;
            }

            let fav_selected = self.filter_mode == super::super::StationFilterMode::Favourites;
            let fav_btn = egui::Button::new(
                RichText::new(format!("★ Избранное {fav_count}"))
                    .color(if fav_selected {
                        Color32::WHITE
                    } else {
                        GOLD_STAR
                    })
                    .strong(),
            )
            .corner_radius(CornerRadius::same(12))
            .stroke(if fav_selected {
                Stroke::NONE
            } else {
                Stroke::new(1.0, BORDER)
            })
            .fill(if fav_selected { ACCENT } else { PANEL });
            if ui.add(fav_btn).clicked() {
                self.filter_mode = if fav_selected {
                    super::super::StationFilterMode::All
                } else {
                    super::super::StationFilterMode::Favourites
                };
            }

            let hist_selected = self.filter_mode == super::super::StationFilterMode::History;
            let hist_btn = egui::Button::new(
                RichText::new(format!("История {hist_count}"))
                    .color(if hist_selected { Color32::WHITE } else { FG })
                    .strong(),
            )
            .corner_radius(CornerRadius::same(12))
            .stroke(if hist_selected {
                Stroke::NONE
            } else {
                Stroke::new(1.0, BORDER)
            })
            .fill(if hist_selected { ACCENT } else { PANEL });
            if ui.add(hist_btn).clicked() {
                self.filter_mode = if hist_selected {
                    super::super::StationFilterMode::All
                } else {
                    super::super::StationFilterMode::History
                };
            }

            if hist_selected
                && hist_count > 0
                && ui
                    .button(RichText::new("✕ Очистить").size(11.0).color(MUTED))
                    .on_hover_text("Очистить историю воспроизведения на этом устройстве")
                    .clicked()
                && let Some(store) = self.personal_data.as_mut()
            {
                let _ = store.clear_history();
            }

            ui.add_space(4.0);
            ui.label(RichText::new("|").color(BORDER));
            ui.add_space(4.0);

            for genre in [
                "Classic Rock",
                "Metal",
                "Hard Rock",
                "Alternative",
                "Punk",
                "Progressive",
                "Indie",
            ] {
                let selected = self.filter_mode == super::super::StationFilterMode::All
                    && self.selected_genre.as_deref() == Some(genre);
                let genre_btn = egui::Button::new(
                    RichText::new(genre)
                        .color(if selected { Color32::WHITE } else { MUTED })
                        .size(12.0)
                        .strong(),
                )
                .corner_radius(CornerRadius::same(12))
                .stroke(if selected {
                    Stroke::NONE
                } else {
                    Stroke::new(1.0, BORDER)
                })
                .fill(if selected { ACCENT } else { PANEL });
                if ui.add(genre_btn).clicked() {
                    self.filter_mode = super::super::StationFilterMode::All;
                    self.selected_genre = (!selected).then(|| genre.to_owned());
                }
            }
        });
        ui.add_space(6.0);

        if return_home {
            self.station_search.clear();
            self.selected_genre = None;
            self.filter_mode = super::super::StationFilterMode::All;
            self.refresh_stations();
        } else if search_requested {
            let query = self.global_station_query();
            if query.is_empty() {
                self.refresh_stations();
            } else {
                self.search_stations(query);
            }
        }

        let mut should_play = false;
        let mut clicked_station: Option<usize> = None;
        let mut toggle_fav: Option<crate::stations::Station> = None;
        let scroll_h = (list_h - 136.0).max(100.0);
        let col_station = t.col_station;
        let col_tags = t.col_tags;
        let col_country = t.col_country;
        let col_bitrate = t.col_bitrate;
        let loading_stations = t.loading_stations;
        let list_empty = t.list_empty;
        let is_loading = self.loading_stations;
        panel(ui, |ui| {
            let full_w = ui.available_width();
            let col_name_x = 68.0;
            let play_col_w = ROW_PLAY_BTN + ROW_PAD_RIGHT + 10.0;
            let country_w = COUNTRY_COL_W;
            let meta_w = META_COL_MIN;
            let fixed_w = col_name_x + meta_w + country_w + play_col_w;
            let name_tags_w = (full_w - fixed_w).max(NAME_COL_MIN + TAGS_COL_MIN);

            let default_name_w: f32 = (name_tags_w * 0.44)
                .clamp(NAME_COL_MIN, NAME_COL_MAX)
                .min(name_tags_w - TAGS_COL_MIN);
            let mut name_w = self
                .station_name_col_w
                .unwrap_or(default_name_w)
                .clamp(NAME_COL_MIN, NAME_COL_MAX.min(name_tags_w - TAGS_COL_MIN));
            let mut tags_w = self
                .station_tags_col_w
                .unwrap_or(name_tags_w - name_w)
                .clamp(TAGS_COL_MIN, name_tags_w - name_w);
            if name_w + tags_w > name_tags_w {
                tags_w = (name_tags_w - name_w).max(TAGS_COL_MIN);
            }

            let col_tags_x = col_name_x + name_w;
            let col_meta_x = col_tags_x + tags_w;
            let col_country_x = col_meta_x + meta_w;
            let top = ui.cursor().top();

            let left_handle = Rect::from_min_max(
                Pos2::new(
                    ui.min_rect().left() + col_tags_x - COL_RESIZE_HIT_W * 0.5,
                    top,
                ),
                Pos2::new(
                    ui.min_rect().left() + col_tags_x + COL_RESIZE_HIT_W * 0.5,
                    top + scroll_h + 28.0,
                ),
            );
            let right_handle = Rect::from_min_max(
                Pos2::new(
                    ui.min_rect().left() + col_meta_x - COL_RESIZE_HIT_W * 0.5,
                    top,
                ),
                Pos2::new(
                    ui.min_rect().left() + col_meta_x + COL_RESIZE_HIT_W * 0.5,
                    top + scroll_h + 28.0,
                ),
            );
            let left_resp = ui.interact(
                left_handle,
                ui.id().with("station_col_resize_left"),
                Sense::drag(),
            );
            let right_resp = ui.interact(
                right_handle,
                ui.id().with("station_col_resize_right"),
                Sense::drag(),
            );
            if left_resp.hovered()
                || left_resp.dragged()
                || right_resp.hovered()
                || right_resp.dragged()
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if left_resp.dragged() {
                let new_name = (name_w + left_resp.drag_delta().x)
                    .clamp(NAME_COL_MIN, NAME_COL_MAX.min(name_tags_w - TAGS_COL_MIN));
                self.station_name_col_w = Some(new_name);
                name_w = new_name;
                tags_w = tags_w.clamp(TAGS_COL_MIN, (name_tags_w - name_w).max(TAGS_COL_MIN));
                self.station_tags_col_w = Some(tags_w);
            }
            if right_resp.dragged() {
                let new_tags = (tags_w + right_resp.drag_delta().x)
                    .clamp(TAGS_COL_MIN, (name_tags_w - name_w).max(TAGS_COL_MIN));
                self.station_tags_col_w = Some(new_tags);
                tags_w = new_tags;
            }

            {
                let (head_rect, _) =
                    ui.allocate_exact_size(Vec2::new(full_w, 28.0), Sense::hover());
                let y = head_rect.center().y;
                let header_font = FontId::proportional(FS_SMALL);
                ui.painter()
                    .rect_filled(head_rect, CornerRadius::same(4), PANEL_2);
                ui.painter().text(
                    Pos2::new(head_rect.left() + 16.0, y),
                    egui::Align2::CENTER_CENTER,
                    "★",
                    FontId::proportional(12.0),
                    MUTED,
                );
                ui.painter().text(
                    Pos2::new(head_rect.left() + col_name_x, y),
                    egui::Align2::LEFT_CENTER,
                    col_station,
                    header_font.clone(),
                    MUTED,
                );
                ui.painter().text(
                    Pos2::new(head_rect.left() + col_tags_x, y),
                    egui::Align2::LEFT_CENTER,
                    col_tags,
                    header_font.clone(),
                    MUTED,
                );
                ui.painter().text(
                    Pos2::new(head_rect.left() + col_meta_x, y),
                    egui::Align2::LEFT_CENTER,
                    col_bitrate,
                    header_font.clone(),
                    MUTED,
                );
                ui.painter().text(
                    Pos2::new(head_rect.left() + col_country_x + country_w * 0.5, y),
                    egui::Align2::CENTER_CENTER,
                    col_country,
                    header_font.clone(),
                    MUTED,
                );
                let bar_reserve = ui.spacing().scroll.floating_allocated_width;
                ui.painter().text(
                    Pos2::new(
                        head_rect.left() + full_w
                            - bar_reserve
                            - ROW_PAD_RIGHT
                            - ROW_PLAY_BTN * 0.5,
                        y,
                    ),
                    egui::Align2::CENTER_CENTER,
                    "▶",
                    FontId::proportional(11.0),
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
                    let row_w = ui.available_width();

                    let visible_indices: Vec<usize> = match self.filter_mode {
                        super::super::StationFilterMode::All => (0..self.stations.len())
                            .filter(|&i| {
                                if let Some(genre) = &self.selected_genre {
                                    let tags = self.stations[i].tags.to_lowercase();
                                    let name = self.stations[i].name.to_lowercase();
                                    let g = genre.to_lowercase();
                                    let g_norm = g.replace('-', " ");
                                    tags.contains(&g)
                                        || tags.contains(&g_norm)
                                        || name.contains(&g)
                                        || name.contains(&g_norm)
                                } else {
                                    true
                                }
                            })
                            .collect(),
                        super::super::StationFilterMode::Favourites => (0..self.stations.len())
                            .filter(|&i| self.is_station_favourite(&self.stations[i].id))
                            .collect(),
                        super::super::StationFilterMode::History => {
                            let history = self
                                .personal_data
                                .as_ref()
                                .map(|s| s.history().to_vec())
                                .unwrap_or_default();
                            let mut indices = Vec::new();
                            for entry in history {
                                if let Some(idx) = crate::personal_data::station_index_by_id(
                                    &self.stations,
                                    &entry.station_id,
                                ) && !indices.contains(&idx)
                                {
                                    indices.push(idx);
                                }
                            }
                            indices
                        }
                    };

                    if visible_indices.is_empty() {
                        let empty_text = match self.filter_mode {
                            super::super::StationFilterMode::Favourites => {
                                "В избранном пока нет станций. Нажмите ★ рядом со станцией в списке."
                            }
                            super::super::StationFilterMode::History => {
                                "История прослушиваний пока пуста."
                            }
                            super::super::StationFilterMode::All => {
                                if is_loading {
                                    loading_stations
                                } else {
                                    list_empty
                                }
                            }
                        };
                        ui.allocate_ui_with_layout(
                            Vec2::new(full_w, scroll_h - 8.0),
                            Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                ui.label(RichText::new(empty_text).color(MUTED).size(14.0));
                            },
                        );
                        return;
                    }

                    for &i in &visible_indices {
                        let st = &self.stations[i];
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
                        let tags_limit = ((tags_w / 6.8) as usize).clamp(18, 72);
                        let meta_limit = ((meta_w / 6.4) as usize).clamp(10, 28);
                        let tags = truncate(&st.tags, tags_limit);
                        let country = format_country_code(&st.country);
                        let meta = truncate(&meta, meta_limit);

                        let (row_rect, resp) =
                            ui.allocate_exact_size(Vec2::new(row_w, ROW_H), Sense::click());
                        if self.scroll_to_station == Some(i) {
                            ui.scroll_to_rect(row_rect, Some(Align::Center));
                        }
                        let mut star_clicked = false;
                        let mut play_clicked = false;
                        if ui.is_rect_visible(row_rect) {
                            let is_current = self.station_now == st.name;
                            let is_playing_this = is_current && self.playing;
                            let bg = if is_playing_this {
                                Color32::from_rgba_unmultiplied(229, 96, 32, 46)
                            } else if selected {
                                Color32::from_rgba_unmultiplied(229, 96, 32, 42)
                            } else if resp.hovered() {
                                Color32::from_rgb(0x27, 0x1f, 0x1a)
                            } else if i % 2 == 1 {
                                Color32::from_rgb(0x1e, 0x17, 0x12)
                            } else {
                                Color32::from_rgb(0x18, 0x13, 0x0f)
                            };
                            ui.painter()
                                .rect_filled(row_rect, CornerRadius::same(4), bg);

                            if selected || is_current {
                                let bar_rect = Rect::from_min_max(
                                    Pos2::new(row_rect.left(), row_rect.top()),
                                    Pos2::new(row_rect.left() + 3.5, row_rect.bottom()),
                                );
                                ui.painter().rect_filled(bar_rect, CornerRadius::ZERO, ACCENT);
                            }

                            let text_color = if is_current {
                                ACCENT
                            } else {
                                FG
                            };
                            let muted_color = MUTED;
                            let y = row_rect.center().y;

                            // 1-Click Interactive Star Button
                            let star_rect = Rect::from_center_size(
                                Pos2::new(row_rect.left() + 16.0, y),
                                Vec2::splat(22.0),
                            );
                            let star_resp = ui.interact(
                                star_rect,
                                ui.id().with(("station_star", &st.id)),
                                Sense::click(),
                            );
                            let is_fav = self.is_station_favourite(&st.id);
                            let (star_icon, star_color) = if is_fav {
                                ("★", GOLD_STAR)
                            } else if star_resp.hovered() {
                                ("☆", FG)
                            } else {
                                ("☆", MUTED.gamma_multiply(0.4))
                            };
                            ui.painter().text(
                                star_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                star_icon,
                                egui::FontId::proportional(14.0),
                                star_color,
                            );
                            if star_resp.clicked() {
                                star_clicked = true;
                                toggle_fav = Some(st.clone());
                            }

                            // Station Logo or 2-letter monogram tile with rock brand palette
                            let icon_rect = Rect::from_center_size(
                                Pos2::new(row_rect.left() + 40.0, y),
                                Vec2::splat(26.0),
                            );
                            let mut has_image = false;
                            if let Some(source) = crate::station_icons::source_url(
                                st,
                                Some(self.rockserver.base_url()),
                            ) {
                                let request_key = crate::station_icons::request_key(st, &source);
                                if let Some(texture) = self.station_icons.get(&request_key) {
                                    has_image = true;
                                    ui.painter().image(
                                        texture.id(),
                                        icon_rect,
                                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                        Color32::WHITE,
                                    );
                                }
                            }
                            if !has_image {
                                let chars: Vec<char> =
                                    st.name.chars().filter(|c| c.is_alphanumeric()).collect();
                                let initial = if chars.len() >= 2 {
                                    format!("{}{}", chars[0], chars[1]).to_uppercase()
                                } else if let Some(c) = chars.first() {
                                    c.to_uppercase().to_string()
                                } else {
                                    "?".to_string()
                                };
                                let monogram_fill = station_color(&st.name);
                                ui.painter().rect_filled(
                                    icon_rect,
                                    CornerRadius::same(5),
                                    monogram_fill,
                                );
                                ui.painter().rect_stroke(
                                    icon_rect,
                                    CornerRadius::same(5),
                                    Stroke::new(1.0, Color32::from_rgb(0x4a, 0x38, 0x2c)),
                                    StrokeKind::Inside,
                                );
                                ui.painter().text(
                                    icon_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    initial,
                                    egui::FontId::proportional(11.0),
                                    Color32::from_rgb(0xec, 0xe4, 0xdc),
                                );
                            }

                            let name_start_x = row_rect.left() + col_name_x;
                            // Reserve room for the animated equalizer so it never
                            // bleeds into the tags column.
                            let eq_room = if is_playing_this { 34.0 } else { 0.0 };
                            let name_limit = (((name_w - eq_room) / 7.5) as usize).clamp(16, 64);
                            let name_text = truncate(&st.name, name_limit);
                            let name_size = ui.painter().text(
                                Pos2::new(name_start_x, y),
                                egui::Align2::LEFT_CENTER,
                                &name_text,
                                FontId::proportional(FS_ROW),
                                text_color,
                            );
                            if is_playing_this {
                                // Animated 4-bar "now playing" equalizer right after the name.
                                let now = ui.input(|i| i.time) as f32;
                                let eq_x = name_start_x + name_size.width() + 10.0;
                                let (bar_w, gap, max_h) = (3.0, 2.5, 14.0);
                                for bar in 0..4 {
                                    let phase = now * (4.2 + bar as f32 * 1.35) + bar as f32 * 1.1;
                                    let h = max_h * (0.35 + 0.65 * (0.5 + 0.5 * phase.sin()));
                                    let x0 = eq_x + bar as f32 * (bar_w + gap);
                                    ui.painter().rect_filled(
                                        Rect::from_min_max(
                                            Pos2::new(x0, y - h * 0.5),
                                            Pos2::new(x0 + bar_w, y + h * 0.5),
                                        ),
                                        CornerRadius::same(1),
                                        ACCENT,
                                    );
                                }
                            }

                            ui.painter().text(
                                Pos2::new(row_rect.left() + col_tags_x, y),
                                egui::Align2::LEFT_CENTER,
                                tags,
                                egui::FontId::proportional(12.0),
                                muted_color,
                            );
                            ui.painter().text(
                                Pos2::new(row_rect.left() + col_meta_x, y),
                                egui::Align2::LEFT_CENTER,
                                meta,
                                egui::FontId::proportional(11.5),
                                muted_color,
                            );

                            // Country badge
                            let country_rect = Rect::from_center_size(
                                Pos2::new(row_rect.left() + col_country_x + country_w * 0.5, y),
                                Vec2::new(26.0, 16.0),
                            );
                            ui.painter().rect_filled(
                                country_rect,
                                CornerRadius::same(3),
                                PANEL_2,
                            );
                            ui.painter().rect_stroke(
                                country_rect,
                                CornerRadius::same(3),
                                Stroke::new(1.0, BORDER),
                                StrokeKind::Inside,
                            );
                            ui.painter().text(
                                country_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                &country,
                                egui::FontId::proportional(10.0),
                                muted_color,
                            );

                            // 1-Click Play/Pause Button on each row — always visible
                            let play_btn_rect = Rect::from_center_size(
                                Pos2::new(
                                    row_rect.left()
                                        + row_w
                                        - ROW_PAD_RIGHT
                                        - ROW_PLAY_BTN * 0.5,
                                    y,
                                ),
                                Vec2::splat(ROW_PLAY_BTN),
                            );
                            let play_resp = ui.interact(
                                play_btn_rect,
                                ui.id().with(("row_play", &st.id)),
                                Sense::click(),
                            );
                            let hovered_or_playing = is_playing_this || play_resp.hovered();
                            let (play_fill, play_fg, play_stroke) = if hovered_or_playing {
                                (ACCENT, Color32::WHITE, Stroke::NONE)
                            } else {
                                (
                                    PANEL_2,
                                    Color32::from_rgb(0xd5, 0xca, 0xc0),
                                    Stroke::new(1.0, BORDER),
                                )
                            };
                            ui.painter().circle_filled(
                                play_btn_rect.center(),
                                ROW_PLAY_BTN * 0.5,
                                play_fill,
                            );
                            if play_stroke.width > 0.0 {
                                ui.painter().circle_stroke(
                                    play_btn_rect.center(),
                                    ROW_PLAY_BTN * 0.5,
                                    play_stroke,
                                );
                            }
                            let play_icon = if is_playing_this { "⏸" } else { "▶" };
                            let x_off = if is_playing_this { 0.0 } else { 1.0 };
                            ui.painter().text(
                                Pos2::new(
                                    play_btn_rect.center().x + x_off,
                                    play_btn_rect.center().y,
                                ),
                                egui::Align2::CENTER_CENTER,
                                play_icon,
                                FontId::proportional(13.0),
                                play_fg,
                            );
                            if play_resp.clicked() {
                                play_clicked = true;
                                clicked_station = Some(i);
                                if is_playing_this {
                                    self.stop();
                                } else {
                                    should_play = true;
                                }
                            }
                        }

                        if resp.clicked() && !star_clicked && !play_clicked {
                            clicked_station = Some(i);
                        }
                        if resp.double_clicked() && !star_clicked && !play_clicked {
                            clicked_station = Some(i);
                            should_play = true;
                        }
                    }
                });

            let guide_color = if left_resp.dragged() || right_resp.dragged() {
                ACCENT.gamma_multiply(0.9)
            } else {
                Color32::from_rgba_unmultiplied(255, 255, 255, 18)
            };
            let guide_top = ui.min_rect().top() + 2.0;
            let guide_bottom = ui.min_rect().top() + scroll_h + 24.0;
            ui.painter().vline(
                ui.min_rect().left() + col_tags_x,
                guide_top..=guide_bottom,
                Stroke::new(1.0, guide_color),
            );
            ui.painter().vline(
                ui.min_rect().left() + col_meta_x,
                guide_top..=guide_bottom,
                Stroke::new(1.0, guide_color),
            );
        });

        if let Some(st) = toggle_fav {
            self.toggle_station_favourite(&st);
        }

        if let Some(i) = clicked_station {
            let prev = self.selected_station;
            self.selected_station = Some(i);
            self.scroll_to_station = Some(i);
            if let Some(s) = self.stations.get(i) {
                log::info!(
                    "station selected: idx={i} (was {prev:?}) name='{}' url={} auto_play={should_play}",
                    s.name,
                    s.url
                );
            }
            self.mark_settings_dirty();
        }
        if clicked_station.is_some() || self.scroll_to_station == self.selected_station {
            self.scroll_to_station = None;
        }
        if should_play {
            log::info!("station double-click → play()");
            self.play();
        }
    }

    fn global_station_query(&self) -> String {
        let mut terms = self.station_search.trim().to_owned();
        if let Some(genre) = &self.selected_genre
            && !terms.to_lowercase().contains(&genre.to_lowercase())
        {
            if !terms.is_empty() {
                terms.push(' ');
            }
            terms.push_str(genre);
        }
        terms
    }
}
