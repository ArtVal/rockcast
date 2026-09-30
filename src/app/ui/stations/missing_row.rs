//! Station row models, missing favourite resolution row, and row list construction.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::app::{
    RockCastApp, StationFilterMode,
    theme::{
        ACCENT, BORDER, FS_ROW, GOLD_STAR, MUTED, PANEL_2, ROW_H, ROW_PLAY_BTN,
        station_color, truncate,
    },
};

use super::table_header::TableLayout;

/// One rendered row of the station table. `Missing` is a favourite whose
/// station is not in the currently loaded list: shown muted, removable via
/// its star, but not playable.
pub(super) enum StationRow {
    Loaded(usize),
    Missing { station_id: String, name: String },
}

pub(super) enum MissingRowAction {
    None,
    Unfavourite,
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
                let mut indices = Vec::new();
                for entry in history {
                    if let Some(idx) = crate::personal_data::station_index_by_id(
                        &self.stations,
                        &entry.station_id,
                    ) && !indices.contains(&idx)
                    {
                        if matches_facets(idx) {
                            indices.push(idx);
                        }
                    }
                }
                indices.into_iter().map(StationRow::Loaded).collect()
            }
        }
    }
}

pub(super) fn draw_missing_favourite_row(
    ui: &mut Ui,
    layout: &TableLayout,
    row_w: f32,
    row_pos: usize,
    station_id: &str,
    name: &str,
    busy: bool,
) -> MissingRowAction {
    let (row_rect, resp) = ui.allocate_exact_size(Vec2::new(row_w, ROW_H), Sense::click());
    if !ui.is_rect_visible(row_rect) {
        return MissingRowAction::None;
    }
    let bg = if resp.hovered() {
        Color32::from_rgb(0x27, 0x1f, 0x1a)
    } else if row_pos % 2 == 1 {
        Color32::from_rgb(0x1e, 0x17, 0x12)
    } else {
        Color32::from_rgb(0x18, 0x13, 0x0f)
    };
    ui.painter().rect_filled(row_rect, CornerRadius::same(4), bg);
    let y = row_rect.center().y;

    // Filled star; clicking removes the favourite even without a station row.
    let star_rect =
        Rect::from_center_size(Pos2::new(row_rect.left() + 16.0, y), Vec2::splat(22.0));
    let star_resp = ui.interact(
        star_rect,
        ui.id().with(("station_star", station_id)),
        Sense::click(),
    );
    ui.painter().text(
        star_rect.center(),
        egui::Align2::CENTER_CENTER,
        "★",
        egui::FontId::proportional(14.0),
        GOLD_STAR,
    );
    if star_resp.clicked() {
        return MissingRowAction::Unfavourite;
    }

    let icon_rect =
        Rect::from_center_size(Pos2::new(row_rect.left() + 40.0, y), Vec2::splat(26.0));
    let chars: Vec<char> = name.chars().filter(|c| c.is_alphanumeric()).collect();
    let initial = if chars.len() >= 2 {
        format!("{}{}", chars[0], chars[1]).to_uppercase()
    } else if let Some(c) = chars.first() {
        c.to_uppercase().to_string()
    } else {
        "?".to_string()
    };
    ui.painter().rect_filled(
        icon_rect,
        CornerRadius::same(5),
        station_color(name).gamma_multiply(0.55),
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
        MUTED,
    );

    let name_limit = ((layout.name_w / 7.5) as usize).clamp(16, 64);
    ui.painter().text(
        Pos2::new(row_rect.left() + layout.col_name_x, y),
        egui::Align2::LEFT_CENTER,
        truncate(name, name_limit),
        FontId::proportional(FS_ROW),
        MUTED,
    );
    ui.painter().text(
        Pos2::new(row_rect.left() + layout.col_tags_x, y),
        egui::Align2::LEFT_CENTER,
        "станция недоступна в текущем списке",
        egui::FontId::proportional(12.0),
        MUTED.gamma_multiply(0.85),
    );
    let play_btn_rect = Rect::from_center_size(
        Pos2::new(row_rect.left() + layout.play_center_x, y),
        Vec2::splat(ROW_PLAY_BTN),
    );
    let play_resp = ui.interact(
        play_btn_rect,
        ui.id().with(("row_play_missing", station_id)),
        Sense::click(),
    );
    let (play_fill, play_fg, play_stroke) = if busy {
        (PANEL_2, MUTED.gamma_multiply(0.5), Stroke::new(1.0, BORDER))
    } else if play_resp.hovered() {
        (ACCENT, Color32::WHITE, Stroke::NONE)
    } else {
        (PANEL_2, MUTED, Stroke::new(1.0, BORDER))
    };
    ui.painter()
        .circle_filled(play_btn_rect.center(), ROW_PLAY_BTN * 0.5, play_fill);
    if play_stroke.width > 0.0 {
        ui.painter()
            .circle_stroke(play_btn_rect.center(), ROW_PLAY_BTN * 0.5, play_stroke);
    }
    ui.painter().text(
        play_btn_rect.center(),
        egui::Align2::CENTER_CENTER,
        "▶",
        egui::FontId::proportional(13.0),
        play_fg,
    );
    if busy {
        resp.on_hover_text(format!("«{name}»: ищу станцию в каталоге и RockServer…"));
        return MissingRowAction::None;
    }
    if play_resp.clicked() || resp.clicked() || resp.double_clicked() {
        return MissingRowAction::Play;
    }
    resp.on_hover_text(format!(
        "«{name}» сейчас не загружена в список станций. Клик или ▶ загрузит её по ID и включит. Звезда убирает её из избранного."
    ));
    MissingRowAction::None
}
