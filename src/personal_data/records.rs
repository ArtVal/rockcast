//! Personal-profile record types, catalog resolution, and shared helpers
//! (RM-007-A profile v1; `updated_at`/history-metadata passthrough are the
//! RM-012-B sync seams).

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Write,
    path::Path,
};

use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::stations::Station;

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("unsupported personal profile schema {0}")]
    UnsupportedSchema(u32),
    #[error("invalid personal profile: {0}")]
    Invalid(String),
    #[error("personal profile I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("personal profile JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalProfile {
    pub schema_version: u32,
    pub profile_id: Uuid,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub favourites: Vec<Favourite>,
    #[serde(default)]
    pub playback_history: Vec<PlaybackHistoryEntry>,
    #[serde(default)]
    pub unresolved_references: Vec<UnresolvedStationReference>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Favourite {
    pub record_id: Uuid,
    pub station_id: String,
    pub added_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub metadata: DisplayMetadata,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackHistoryEntry {
    pub record_id: Uuid,
    pub station_id: String,
    pub started_at: String,
    pub last_played_at: String,
    /// Server-sync version field (RM-012-B). Older v1 files predate it and are
    /// backfilled from `last_played_at` when the profile opens.
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub play_duration_ms: Option<u64>,
    #[serde(default)]
    pub metadata: HistoryMetadata,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DisplayMetadata {
    #[serde(default)]
    pub last_known_name: Option<String>,
    #[serde(default)]
    pub catalog_version: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMetadata {
    #[serde(default)]
    pub last_known_name: Option<String>,
    #[serde(default)]
    pub catalog_version: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    /// Verbatim catch-all for metadata keys written by other RockCast clients;
    /// keeps server-synced objects byte-stable across pull/push round trips.
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedStationReference {
    pub reference_id: Uuid,
    pub source_kind: String,
    pub original_station_id: String,
    pub first_seen_at: String,
    pub reason: String,
    #[serde(default)]
    pub candidate_station_ids: Vec<String>,
    #[serde(default)]
    pub last_known_name: Option<String>,
    #[serde(default)]
    pub catalog_version: Option<String>,
}

impl LocalProfile {
    pub(crate) fn new(timestamp: String) -> Self {
        Self {
            schema_version: 1,
            profile_id: Uuid::new_v4(),
            created_at: timestamp.clone(),
            updated_at: timestamp,
            favourites: vec![],
            playback_history: vec![],
            unresolved_references: vec![],
            metadata: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TombstoneReason {
    Removed,
    Merged,
    Split,
}
#[derive(Debug, Clone)]
pub struct Tombstone {
    pub id: String,
    pub reason: TombstoneReason,
    pub replacement_ids: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct CatalogResolver {
    active: HashSet<String>,
    legacy: HashMap<String, String>,
    tombstones: HashMap<String, Tombstone>,
    pub catalog_version: Option<String>,
}

impl CatalogResolver {
    pub fn from_stations(stations: &[Station], catalog_version: Option<String>) -> Self {
        let mut active = HashSet::new();
        let mut legacy = HashMap::new();
        for station in stations {
            active.insert(station.id.clone());
            for id in &station.legacy_ids {
                legacy
                    .entry(id.clone())
                    .or_insert_with(|| station.id.clone());
                if let Some(hash) = id.strip_prefix("rockmobile:rockcast-") {
                    legacy
                        .entry(format!("legacy-{hash}"))
                        .or_insert_with(|| station.id.clone());
                }
            }
        }
        Self {
            active,
            legacy,
            tombstones: HashMap::new(),
            catalog_version,
        }
    }
    pub fn with_tombstones(mut self, tombstones: Vec<Tombstone>) -> Self {
        self.tombstones = tombstones.into_iter().map(|t| (t.id.clone(), t)).collect();
        self
    }
    pub(super) fn resolve(&self, id: &str) -> Result<String, (String, Vec<String>)> {
        if !valid_station_id(id) {
            return Err(("legacy-unmapped".into(), vec![]));
        }
        if self.active.contains(id) {
            return Ok(id.into());
        }
        if let Some(target) = self.legacy.get(id) {
            return Ok(target.clone());
        }
        let mut current = id;
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(current.to_owned()) {
                return Err(("missing".into(), vec![]));
            }
            let Some(tombstone) = self.tombstones.get(current) else {
                return Err((
                    if id.starts_with("legacy-") || id.starts_with("rockserver-") {
                        "legacy-unmapped"
                    } else {
                        "missing"
                    }
                    .into(),
                    vec![],
                ));
            };
            match tombstone.reason {
                TombstoneReason::Removed => return Err(("removed".into(), vec![])),
                TombstoneReason::Split => {
                    return Err(("split".into(), tombstone.replacement_ids.clone()));
                }
                TombstoneReason::Merged if tombstone.replacement_ids.len() == 1 => {
                    current = &tombstone.replacement_ids[0];
                    if self.active.contains(current) {
                        return Ok(current.into());
                    }
                }
                TombstoneReason::Merged => return Err(("missing".into(), vec![])),
            }
        }
    }
}

/// Finds a persisted station identity in the currently loaded catalog.
///
/// Personal-data UI uses the returned catalog index and delegates playback to
/// the same app path as the main station list.
pub(crate) fn station_index_by_id(stations: &[Station], station_id: &str) -> Option<usize> {
    stations.iter().position(|station| station.id == station_id)
}

pub(crate) fn display(station: &Station, catalog_version: Option<String>) -> DisplayMetadata {
    DisplayMetadata {
        last_known_name: Some(station.name.clone()),
        catalog_version,
    }
}
fn valid_station_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 96
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
        && !id.contains("--")
}
pub(crate) fn now() -> Result<String, ProfileError> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| ProfileError::Invalid(e.to_string()))
}
pub(crate) fn parse_time(value: &str) -> Result<OffsetDateTime, ProfileError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|e| ProfileError::Invalid(e.to_string()))
}
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ProfileError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    };
    let temporary = path.with_extension("tmp");
    let mut file = File::create(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    replace_file(&temporary, path)?;
    Ok(())
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
    std::fs::rename(source, destination)
}
