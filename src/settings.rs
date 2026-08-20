//! Persist volume and selected station across launches.

use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Speech-recognition transport requested for RockServer voice sessions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RockServerVoiceMode {
    /// Existing REST request submitted after recording stops.
    #[default]
    BufferedV1,
    /// SpeechKit v3 receives microphone chunks while recording.
    StreamingV3,
}

impl RockServerVoiceMode {
    pub const fn protocol_value(self) -> &'static str {
        match self {
            Self::BufferedV1 => "buffered_v1",
            Self::StreamingV3 => "streaming_v3",
        }
    }
}

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
    /// Most recently started station, used by the voice command "play music".
    #[serde(default)]
    pub last_played_station: Option<crate::stations::Station>,
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
    /// Enables RockServer search and voice control; false preserves autonomous behavior.
    #[serde(default)]
    pub rockserver_enabled: bool,
    /// Local or LAN RockServer base URL, never a credential-bearing URL.
    #[serde(default = "default_rockserver_url")]
    pub rockserver_url: String,
    /// Bearer credential sent to RockServer; it is never embedded in the URL.
    #[serde(default)]
    pub rockserver_bearer_token: String,
    /// Recognition transport for the next RockServer voice session.
    #[serde(default)]
    pub rockserver_voice_mode: RockServerVoiceMode,
}

fn default_rockserver_url() -> String {
    "http://127.0.0.1:3000".to_owned()
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

/// Settings, log, and the editable `stations.txt` copy.
///
/// - Windows: `%LOCALAPPDATA%\RockCast`
/// - Unix: `$XDG_CONFIG_HOME/rockcast`, else `~/.config/rockcast`
pub fn app_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("RockCast"))
    }
    #[cfg(not(windows))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(xdg).join("rockcast"));
        }
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config").join("rockcast"))
    }
}

fn settings_path() -> PathBuf {
    app_dir()
        .map(|dir| dir.join("settings.json"))
        .unwrap_or_else(|| PathBuf::from("rockcast_settings.json"))
}

/// App-dir `rockcast.log`, or `./rockcast.log` if no app dir is available.
pub fn log_path() -> PathBuf {
    app_dir()
        .map(|dir| dir.join("rockcast.log"))
        .unwrap_or_else(|| PathBuf::from("rockcast.log"))
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
            rockserver_bearer_token: "test-token".to_owned(),
            ..Default::default()
        };
        expected.save_to(&path).unwrap();
        let actual = AppSettings::load_from(&path).unwrap();
        assert_eq!(actual.volume, 73);
        assert!(actual.cast_relay);
        assert_eq!(actual.rockserver_bearer_token, "test-token");
        assert_eq!(
            actual.rockserver_voice_mode,
            RockServerVoiceMode::BufferedV1
        );
        fs::write(&path, b"{").unwrap();
        assert!(AppSettings::load_from(&path).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn voice_mode_uses_stable_wire_values() {
        assert_eq!(
            RockServerVoiceMode::BufferedV1.protocol_value(),
            "buffered_v1"
        );
        assert_eq!(
            RockServerVoiceMode::StreamingV3.protocol_value(),
            "streaming_v3"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_app_dir_ends_with_rockcast() {
        let dir = app_dir().expect("HOME or XDG_CONFIG_HOME");
        assert_eq!(dir.file_name().and_then(|n| n.to_str()), Some("rockcast"));
    }
}
