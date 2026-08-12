//! Persist volume and selected station across launches.

use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("serialize settings: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("write settings: {0}")]
    Io(#[from] std::io::Error),
}

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
        match Self::load_from(&path) {
            Ok(value) => value,
            Err(e) => {
                if path.exists() {
                    log::warn!("failed to load settings {}: {e}", path.display());
                }
                Self::default()
            }
        }
    }

    pub fn save(&self) -> Result<(), SettingsError> {
        let path = settings_path();
        self.save_to(&path)
    }

    fn load_from(path: &Path) -> Result<Self, SettingsError> {
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    fn save_to(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_vec_pretty(self)?;
        let temporary = path.with_extension("json.tmp");
        let mut file = File::create(&temporary)?;
        file.write_all(&raw)?;
        file.sync_all()?;
        replace_file(&temporary, path)?;
        Ok(())
    }
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both paths are NUL-terminated UTF-16 buffers valid for this call.
    let ok = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_invalid_json_is_reported() {
        let dir = std::env::temp_dir().join(format!("rockcast-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        let expected = AppSettings {
            volume: 73,
            cast_relay: true,
            ..Default::default()
        };
        expected.save_to(&path).unwrap();
        let actual = AppSettings::load_from(&path).unwrap();
        assert_eq!(actual.volume, 73);
        assert!(actual.cast_relay);
        fs::write(&path, b"{").unwrap();
        assert!(AppSettings::load_from(&path).is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
