//! Persist volume and selected station across launches.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppSettings {
    #[serde(default = "default_volume")]
    pub volume: u8,
    #[serde(default)]
    pub station_url: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    /// Parallel stream analysis for the visualizer (extra traffic).
    #[serde(default)]
    pub eq_enabled: bool,
    /// PC fetches the station (VPN) and relays audio to Cast over LAN.
    #[serde(default)]
    pub cast_relay: bool,
    #[serde(default)]
    pub language: crate::i18n::Lang,
}

fn default_volume() -> u8 {
    50
}

impl AppSettings {
    pub fn load() -> Self {
        let path = settings_path();
        match fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let Ok(raw) = serde_json::to_string_pretty(self) else {
            return;
        };
        if let Err(e) = fs::write(&path, raw) {
            log::warn!("failed to save settings {}: {e}", path.display());
        }
    }
}

fn settings_path() -> PathBuf {
    if let Ok(base) = std::env::var("LOCALAPPDATA") {
        return Path::new(&base).join("RockCast").join("settings.json");
    }
    PathBuf::from("rockcast_settings.json")
}

/// `%LOCALAPPDATA%\RockCast\rockcast.log` (or `./rockcast.log`).
pub fn log_path() -> PathBuf {
    if let Ok(base) = std::env::var("LOCALAPPDATA") {
        return Path::new(&base).join("RockCast").join("rockcast.log");
    }
    PathBuf::from("rockcast.log")
}
