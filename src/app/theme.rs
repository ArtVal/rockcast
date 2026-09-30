//! egui theme tokens and layout helpers.

use std::time::Duration;

use eframe::egui::{self, Color32, CornerRadius, Frame, Ui};

pub(crate) const BG: Color32 = Color32::from_rgb(0x15, 0x11, 0x0e);
pub(crate) const PANEL: Color32 = Color32::from_rgb(0x20, 0x19, 0x14);
pub(crate) const PANEL_2: Color32 = Color32::from_rgb(0x2a, 0x21, 0x1b);
pub(crate) const BORDER: Color32 = Color32::from_rgb(0x3b, 0x2c, 0x23);
pub(crate) const BORDER_SUBTLE: Color32 = Color32::from_rgb(0x2d, 0x21, 0x1a);
pub(crate) const FG: Color32 = Color32::from_rgb(0xf3, 0xee, 0xe9);
pub(crate) const ACCENT: Color32 = Color32::from_rgb(0xe5, 0x60, 0x20);
pub(crate) const MUTED: Color32 = Color32::from_rgb(0x9e, 0x8d, 0x82);
pub(crate) const BAR_DIM: Color32 = Color32::from_rgb(0x5a, 0x40, 0x30);
pub(crate) const GOLD_STAR: Color32 = Color32::from_rgb(0xf5, 0x9e, 0x0b);
pub(crate) const GREEN: Color32 = Color32::from_rgb(0x3d, 0xdc, 0x84);
/// Comfortable station row height (two 30px controls must fit with air).
pub(crate) const ROW_H: f32 = 44.0;
/// Diameter of the round per-row play/pause buttons.
pub(crate) const ROW_PLAY_BTN: f32 = 30.0;
/// Type scale — the only font sizes the main window may use.
pub(crate) const FS_MICRO: f32 = 10.5; // country badges, tiny captions
pub(crate) const FS_SMALL: f32 = 11.5; // column headers, secondary meta
pub(crate) const FS_BODY: f32 = 12.5; // buttons, tags, status line
pub(crate) const FS_ROW: f32 = 13.5; // station names
pub(crate) const FS_TITLE: f32 = 18.0; // app title, account window title
/// EQ bar animation target rate (~20 FPS).
pub(crate) const EQ_REPAINT_INTERVAL: Duration = Duration::from_millis(50);
/// Background polling while playback/loading is active.
pub(crate) const UI_SLOW_REPAINT_INTERVAL: Duration = Duration::from_millis(120);

/// Grim rock & heavy metal color palette for station monogram badges.
pub(crate) const MONOGRAM_PALETTE: [Color32; 8] = [
    Color32::from_rgb(0x38, 0x22, 0x16), // Dark Burnt Iron
    Color32::from_rgb(0x48, 0x1a, 0x16), // Deep Blood Maroon
    Color32::from_rgb(0x30, 0x28, 0x24), // Heavy Metal Gunmetal
    Color32::from_rgb(0x52, 0x28, 0x14), // Dark Charred Copper
    Color32::from_rgb(0x3d, 0x2c, 0x1d), // Dark Raw Bronze
    Color32::from_rgb(0x44, 0x1b, 0x17), // Dark Crimson Rust
    Color32::from_rgb(0x28, 0x25, 0x23), // Smoked Carbon Steel
    Color32::from_rgb(0x5a, 0x2e, 0x15), // Deep Scorched Amber
];

/// Returns a stable distinct rock brand color for a station monogram based on its name.
pub(crate) fn station_color(name: &str) -> Color32 {
    let mut hash: u32 = 0;
    for b in name.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u32::from(b));
    }
    MONOGRAM_PALETTE[(hash as usize) % MONOGRAM_PALETTE.len()]
}

/// Maps raw country names from radio directories to clean 2-letter uppercase ISO codes.
pub(crate) fn format_country_code(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "—".to_string();
    }
    let lower = trimmed.to_lowercase();
    match lower.as_str() {
        "united states" | "usa" | "us" => "US",
        "germany" | "deutschland" | "de" => "DE",
        "canada" | "ca" => "CA",
        "united kingdom" | "great britain" | "uk" | "gb" | "england" => "GB",
        "russia" | "russian federation" | "ru" => "RU",
        "switzerland" | "suisse" | "schweiz" | "ch" => "CH",
        "sweden" | "sverige" | "se" => "SE",
        "poland" | "polska" | "pl" => "PL",
        "netherlands" | "holland" | "nl" => "NL",
        "france" | "fr" => "FR",
        "italy" | "italia" | "it" => "IT",
        "spain" | "espana" | "es" => "ES",
        "finland" | "suomi" | "fi" => "FI",
        "norway" | "norge" | "no" => "NO",
        "austria" | "osterreich" | "at" => "AT",
        "australia" | "au" => "AU",
        "brazil" | "brasil" | "br" => "BR",
        "ukraine" | "ua" => "UA",
        "czech republic" | "czechia" | "cz" => "CZ",
        "ireland" | "ie" => "IE",
        "belgium" | "be" => "BE",
        "denmark" | "dk" => "DK",
        "greece" | "gr" => "GR",
        "japan" | "jp" => "JP",
        "mexico" | "mx" => "MX",
        "argentina" | "ar" => "AR",
        "chile" | "cl" => "CL",
        "portugal" | "pt" => "PT",
        _ => {
            if trimmed.len() <= 3 {
                return trimmed.to_uppercase();
            }
            let chars: String = trimmed
                .chars()
                .filter(|c| c.is_alphanumeric())
                .take(2)
                .collect();
            if chars.is_empty() {
                "?"
            } else {
                return chars.to_uppercase();
            }
        }
    }
    .to_string()
}

pub(crate) fn panel(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(PANEL)
        .corner_radius(CornerRadius::same(10))
        .stroke(egui::Stroke::new(1.0, BORDER_SUBTLE))
        .inner_margin(egui::Margin::same(12))
        .show(ui, add);
}

pub(crate) fn truncate(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

