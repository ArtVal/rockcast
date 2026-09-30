//! Persist volume and selected station across launches.

use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
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
    /// Most recently started station, used by the voice command "play music".
    #[serde(default)]
    pub last_played_station: Option<crate::stations::Station>,
    #[serde(default)]
    pub device_id: Option<String>,
    /// Last device-control snapshot revision accepted by this RockCast identity.
    #[serde(default)]
    pub device_control_state_revision: u64,
    /// Parallel stream analysis for the visualizer (extra traffic).
    #[serde(default)]
    pub eq_enabled: bool,
    /// PC fetches the station (VPN) and relays audio to Cast over LAN.
    #[serde(default)]
    pub cast_relay: bool,
    #[serde(default)]
    pub language: crate::i18n::Lang,
    #[serde(default)]
    pub station_name_col_w: Option<f32>,
    #[serde(default)]
    pub station_meta_col_w: Option<f32>,
}

enum SaveCommand {
    Save,
    Flush(mpsc::Sender<()>),
}

pub struct SettingsWriter {
    tx: mpsc::SyncSender<SaveCommand>,
    pending: Arc<Mutex<Option<AppSettings>>>,
}

impl SettingsWriter {
    pub fn new() -> Self {
        // One wake-up is enough: the shared slot always contains the latest
        // snapshot, so rapid UI changes coalesce without growing a queue.
        let (tx, rx) = mpsc::sync_channel(1);
        let pending = Arc::new(Mutex::new(None::<AppSettings>));
        let writer_pending = Arc::clone(&pending);
        thread::Builder::new()
            .name("rockcast-settings".into())
            .spawn(move || {
                while let Ok(command) = rx.recv() {
                    match command {
                        SaveCommand::Save => {
                            let settings = { writer_pending.lock().unwrap().take() };
                            if let Some(settings) = settings
                                && let Err(error) = settings.save()
                            {
                                log::warn!("failed to persist settings: {error}");
                            }
                        }
                        SaveCommand::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            })
            .expect("spawn settings writer");
        Self { tx, pending }
    }

    pub fn save(&self, settings: AppSettings) -> Result<(), &'static str> {
        *self.pending.lock().unwrap() = Some(settings);
        match self.tx.try_send(SaveCommand::Save) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::TrySendError::Disconnected(_)) => Err("settings writer stopped"),
        }
    }

    pub fn flush(&self) {
        const TIMEOUT: Duration = Duration::from_secs(2);
        let deadline = Instant::now() + TIMEOUT;
        let (done_tx, done_rx) = mpsc::channel();
        let mut flush = SaveCommand::Flush(done_tx);
        loop {
            match self.tx.try_send(flush) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Full(command)) if Instant::now() < deadline => {
                    flush = command;
                    thread::sleep(Duration::from_millis(5));
                }
                Err(mpsc::TrySendError::Full(_)) => {
                    log::warn!("settings writer queue did not drain within {TIMEOUT:?}");
                    return;
                }
                Err(mpsc::TrySendError::Disconnected(_)) => return,
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if done_rx.recv_timeout(remaining).is_err() {
            log::warn!("settings writer did not flush within {TIMEOUT:?}");
        }
    }
}

impl Default for SettingsWriter {
    fn default() -> Self {
        Self::new()
    }
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
        let raw_value: serde_json::Value = serde_json::from_str(&raw)?;
        let has_legacy_rockserver_settings = raw_value.as_object().is_some_and(|settings| {
            [
                "rockserver_enabled",
                "rockserver_url",
                "rockserver_bearer_token",
                "rockserver_voice_mode",
            ]
            .iter()
            .any(|key| settings.contains_key(*key))
        });
        let value: Self = serde_json::from_value(raw_value)?;
        if has_legacy_rockserver_settings && let Err(error) = value.save_to(path) {
            log::warn!("failed to remove legacy RockServer settings: {error}");
        }
        Ok(value)
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

/// Settings, log, and the editable `stations.v1.json` copy.
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

    #[test]
    fn legacy_rockserver_credentials_are_not_loaded_or_saved() {
        let dir =
            std::env::temp_dir().join(format!("rockcast-legacy-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        fs::create_dir_all(&dir).unwrap();
        let legacy = r#"{
            "volume": 61,
            "rockserver_enabled": true,
            "rockserver_url": "http://127.0.0.1:3000",
            "rockserver_bearer_token": "must-not-survive",
            "rockserver_voice_mode": "streaming_v3"
        }"#;
        fs::write(&path, legacy).unwrap();
        let settings = AppSettings::load_from(&path).unwrap();
        assert_eq!(settings.volume, 61);
        let saved = fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("rockserver"));
        assert!(!saved.contains("must-not-survive"));
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_app_dir_ends_with_rockcast() {
        let dir = app_dir().expect("HOME or XDG_CONFIG_HOME");
        assert_eq!(dir.file_name().and_then(|n| n.to_str()), Some("rockcast"));
    }
}
