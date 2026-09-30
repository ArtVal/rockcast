//! Station row models, missing favourite resolution row, and row list construction.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Pos2, RichText, Sense, Stroke, StrokeKind, Vec2,
};
use egui_extras::TableRow;

use crate::app::{
    RockCastApp, StationFilterMode,
    theme::{
        ACCENT, BORDER, FS_ROW, GOLD_STAR, MUTED, PANEL_2, ROW_PLAY_BTN,
        station_color,
    },
};

/// One rendered row of the station table. `Missing` is a favourite whose
/// station is not in the currently loaded list: shown muted, removable via
/// its star, but not playable.
pub(super) enum StationRow {
    Loaded(usize),
    Missing { station_id: String, name: String },
}

pub(super) enum MissingRowAction {
    None,
    ToggleFav(String, String),
    Play,
}

impl RockCastApp {
    pub(in crate::app) fn station_matches_facets(&self, idx: usize) -> bool {
        if let Some(st) = self.stations.get(idx) {
            if let Some(country) = &self.selected_country {
                if !st.country.eq_ignore_ascii_case(country) {
                    return false;
                }
            }
            if let Some(min_bitrate) = self.selected_min_bitrate {
                if st.bitrate > 0 && st.bitrate < min_bitrate {
                    return false;
                }
            }
        }
        true
    }

    pub(super) fn build_station_rows(&self) -> Vec<StationRow> {
        let matches_facets = |idx: usize| -> bool { self.station_matches_facets(idx) };

        match self.filter_mode {
            StationFilterMode::All => (0..self.stations.len())
                .filter(|&idx| matches_facets(idx))
                .map(StationRow::Loaded)
                .collect(),
            StationFilterMode::Favourites => self
                .personal_data
                .as_ref()
                .map(|store| store.favourites().to_vec())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|favourite| {
                    match crate::personal_data::station_index_by_id(
                        &self.stations,
                        &favourite.station_id,
                    ) {
                        Some(index) => {
                            if matches_facets(index) {
                                Some(StationRow::Loaded(index))
                            } else {
                                None
                            }
                        }
                        None => {
                            if self.selected_country.is_none() && self.selected_min_bitrate.is_none() {
                                Some(StationRow::Missing {
                                    station_id: favourite.station_id.clone(),
                                    name: favourite
                                        .metadata
                                        .last_known_name
                                        .clone()
                                        .unwrap_or_else(|| favourite.station_id.clone()),
                                })
                            } else {
                                None
                            }
                        }
                    }
                })
                .collect(),
            StationFilterMode::History => {
                let history = self
                    .personal_data
                    .as_ref()
                    .map(|s| s.history().to_vec())
                    .unwrap_or_default();
                let mut seen_ids = std::collections::HashSet::new();
                let mut rows = Vec::new();
                for entry in history {
                    if !seen_ids.insert(entry.station_id.clone()) {
                        continue;
                    }
                    match crate::personal_data::station_index_by_id(
                        &self.stations,
                        &entry.station_id,
                    ) {
                        Some(idx) => {
                            if matches_facets(idx) {
                                rows.push(StationRow::Loaded(idx));
                            }
                        }
                        None => {
                            if self.selected_country.is_none() && self.selected_min_bitrate.is_none() {
                                rows.push(StationRow::Missing {
                                    station_id: entry.station_id.clone(),
                                    name: entry
                                        .metadata
                                        .last_known_name
                                        .clone()
                                        .unwrap_or_else(|| entry.station_id.clone()),
                                });
                            }
                        }
                    }
                }
                rows
            }
        }
    }
}

pub(super) fn draw_missing_favourite_row(
    row: &mut TableRow<'_, '_>,
    row_pos: usize,
    station_id: &str,
    name: &str,
    is_fav: bool,
    busy: bool,
    table_x_range: egui::Rangef,
) -> MissingRowAction {
    let mut star_clicked = false;
    let mut play_clicked = false;
    let mut row_clicked = false;

    let draw_cell_bg = |ui: &mut egui::Ui, col_idx: usize| -> bool {
        let cell_rect = ui.max_rect();
        let is_row_hovered = ui.ctx().pointer_latest_pos().is_some_and(|p| {
            cell_rect.y_range().contains(p.y) && table_x_range.contains(p.x)
        });
        let bg = if is_row_hovered {
            Color32::from_rgb(0x27, 0x1f, 0x1a)
        } else if row_pos % 2 == 1 {
            Color32::from_rgb(0x1e, 0x17, 0x12)
        } else {
            Color32::from_rgb(0x18, 0x13, 0x0f)
        };
        let corner = if col_idx == 0 {
            CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 }
        } else if col_idx == 5 {
            CornerRadius { nw: 0, sw: 0, ne: 4, se: 4 }
        } else {
            CornerRadius::ZERO
        };
        ui.painter().rect_filled(cell_rect, corner, bg);

        let cell_resp = ui.interact(
            cell_rect,
            ui.id().with(("missing_row_cell", row_pos, col_idx)),
            Sense::click(),
        );
        cell_resp.clicked() || cell_resp.double_clicked()
    };

    // Col 0: Star + Monogram
    row.col(|ui| {
        if draw_cell_bg(ui, 0) {
            row_clicked = true;
        }

        ui.horizontal_centered(|ui| {
            ui.add_space(8.0);

            let (star_icon, star_color) = if is_fav {
                ("★", GOLD_STAR)
            } else {
                ("☆", MUTED.gamma_multiply(0.5))
            };
            let star_btn = egui::Button::new(
                RichText::new(star_icon).size(14.0).color(star_color)
            ).frame(false);
            if ui.add(star_btn).clicked() {
                star_clicked = true;
            }

            ui.add_space(4.0);
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());
            let chars: Vec<char> = name.chars().filter(|c| c.is_alphanumeric()).collect();
            let initial = if chars.len() >= 2 {
                format!("{}{}", chars[0], chars[1]).to_uppercase()
            } else if let Some(c) = chars.first() {
                c.to_uppercase().to_string()
            } else {
                "?".to_string()
            };
            ui.painter().rect_filled(rect, CornerRadius::same(5), station_color(name).gamma_multiply(0.55));
            ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, Color32::from_rgb(0x4a, 0x38, 0x2c)), StrokeKind::Inside);
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initial, FontId::proportional(11.0), Color32::from_rgb(0xec, 0xe4, 0xdc));
        });
    });

    // Col 1: Name (MUTED)
    row.col(|ui| {
        if draw_cell_bg(ui, 1) {
            row_clicked = true;
        }
        ui.horizontal_centered(|ui| {
            ui.add_space(4.0);
            let label = egui::Label::new(RichText::new(name).color(MUTED).size(FS_ROW))
                .sense(Sense::click())
                .truncate();
            let resp = ui.add(label);
            if resp.clicked() || resp.double_clicked() {
                row_clicked = true;
            }
            resp.on_hover_text(name);
        });
    });

    // Col 2: Missing message
    row.col(|ui| {
        if draw_cell_bg(ui, 2) {
            row_clicked = true;
        }
        ui.horizontal_centered(|ui| {
            ui.add_space(4.0);
            let label = egui::Label::new(
                RichText::new("станция недоступна в текущем списке")
                    .color(MUTED.gamma_multiply(0.85))
                    .size(12.0)
            )
            .sense(Sense::click())
            .truncate();
            let resp = ui.add(label);
            if resp.clicked() || resp.double_clicked() {
                row_clicked = true;
            }
        });
    });

    // Col 3: empty
    row.col(|ui| {
        if draw_cell_bg(ui, 3) {
            row_clicked = true;
        }
    });

    // Col 4: empty
    row.col(|ui| {
        if draw_cell_bg(ui, 4) {
            row_clicked = true;
        }
    });

    // Col 5: Play button
    row.col(|ui| {
        if draw_cell_bg(ui, 5) {
            row_clicked = true;
        }
        ui.centered_and_justified(|ui| {
            let (btn_rect, btn_resp) = ui.allocate_exact_size(Vec2::splat(ROW_PLAY_BTN), Sense::click());
            let (fill, fg, stroke) = if busy {
                (PANEL_2, MUTED.gamma_multiply(0.5), Stroke::new(1.0, BORDER))
            } else if btn_resp.hovered() {
                (ACCENT, Color32::WHITE, Stroke::NONE)
            } else {
                (PANEL_2, MUTED, Stroke::new(1.0, BORDER))
            };
            ui.painter().circle_filled(btn_rect.center(), ROW_PLAY_BTN * 0.5, fill);
            if stroke.width > 0.0 {
                ui.painter().circle_stroke(btn_rect.center(), ROW_PLAY_BTN * 0.5, stroke);
            }
            let x_off = 1.0;
            ui.painter().text(
                Pos2::new(btn_rect.center().x + x_off, btn_rect.center().y),
                egui::Align2::CENTER_CENTER,
                "▶",
                FontId::proportional(13.0),
                fg,
            );
            if btn_resp.clicked() {
                play_clicked = true;
            }
        });
    });

    if star_clicked {
        return MissingRowAction::ToggleFav(station_id.to_string(), name.to_string());
    }

    let resp = row.response();
    if busy {
        resp.on_hover_text(format!("«{name}»: ищу станцию в каталоге и RockServer…"));
        return MissingRowAction::None;
    }
    if play_clicked || row_clicked || resp.clicked() || resp.double_clicked() {
        return MissingRowAction::Play;
    }
    resp.on_hover_text(format!(
        "«{name}» сейчас не загружена в список станций. Клик или ▶ загрузит её по ID и включит. Звезда переключает избранное."
    ));
    MissingRowAction::None
}
