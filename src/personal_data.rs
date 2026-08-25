//! Offline-only favourites and playback history (RM-007-A profile v1).

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::stations::Station;

const MAX_FAVOURITES: usize = 500;
const MAX_HISTORY: usize = 500;
const HISTORY_RETENTION_DAYS: i64 = 90;
const COALESCE_SECONDS: i64 = 5 * 60;

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
    fn resolve(&self, id: &str) -> Result<String, (String, Vec<String>)> {
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

pub struct PersonalDataStore {
    path: PathBuf,
    profile: LocalProfile,
    resolver: CatalogResolver,
}
impl PersonalDataStore {
    pub fn open(path: PathBuf, resolver: CatalogResolver) -> Result<Self, ProfileError> {
        let profile = if path.exists() {
            serde_json::from_slice::<LocalProfile>(&fs::read(&path)?)?
        } else {
            LocalProfile::new(now()?)
        };
        if profile.schema_version != 1 {
            return Err(ProfileError::UnsupportedSchema(profile.schema_version));
        }
        let mut store = Self {
            path,
            profile,
            resolver,
        };
        store.migrate_and_retain()?;
        Ok(store)
    }
    pub fn default_path() -> PathBuf {
        crate::settings::app_dir()
            .map(|d| d.join("personal-profile.v1.json"))
            .unwrap_or_else(|| PathBuf::from("rockcast-personal-profile.v1.json"))
    }
    pub fn profile(&self) -> &LocalProfile {
        &self.profile
    }
    pub fn favourites(&self) -> &[Favourite] {
        &self.profile.favourites
    }
    pub fn history(&self) -> &[PlaybackHistoryEntry] {
        &self.profile.playback_history
    }
    pub fn last_played_station_id(&self) -> Option<&str> {
        self.profile
            .playback_history
            .first()
            .map(|entry| entry.station_id.as_str())
    }
    pub fn is_favourite(&self, station_id: &str) -> bool {
        self.profile
            .favourites
            .iter()
            .any(|f| f.station_id == station_id)
    }
    pub fn toggle_favourite(&mut self, station: &Station) -> Result<bool, ProfileError> {
        if self.is_favourite(&station.id) {
            self.profile
                .favourites
                .retain(|f| f.station_id != station.id);
            self.save()?;
            return Ok(false);
        }
        if self.profile.favourites.len() >= MAX_FAVOURITES {
            return Err(ProfileError::Invalid("favourites limit reached".into()));
        }
        let ts = now()?;
        self.profile.favourites.push(Favourite {
            record_id: Uuid::new_v4(),
            station_id: station.id.clone(),
            added_at: ts.clone(),
            updated_at: ts,
            metadata: display(station, self.resolver.catalog_version.clone()),
        });
        self.save()?;
        Ok(true)
    }
    pub fn record_play(&mut self, station: &Station) -> Result<(), ProfileError> {
        let ts = now()?;
        let started = parse_time(&ts)?;
        if let Some(entry) = self
            .profile
            .playback_history
            .iter_mut()
            .max_by_key(|e| parse_time(&e.last_played_at).ok())
            && entry.station_id == station.id
            && started - parse_time(&entry.last_played_at)?
                <= time::Duration::seconds(COALESCE_SECONDS)
        {
            entry.last_played_at = ts.clone();
            entry.ended_at = Some(ts.clone());
            entry.metadata.last_known_name = Some(station.name.clone());
            entry.metadata.catalog_version = self.resolver.catalog_version.clone();
        } else {
            self.profile.playback_history.push(PlaybackHistoryEntry {
                record_id: Uuid::new_v4(),
                station_id: station.id.clone(),
                started_at: ts.clone(),
                last_played_at: ts.clone(),
                ended_at: Some(ts),
                play_duration_ms: Some(0),
                metadata: HistoryMetadata {
                    last_known_name: Some(station.name.clone()),
                    catalog_version: self.resolver.catalog_version.clone(),
                    source: Some("bundled".into()),
                },
            });
        }
        self.retain()?;
        self.save()
    }
    pub fn clear_history(&mut self) -> Result<(), ProfileError> {
        self.profile.playback_history.clear();
        self.profile
            .unresolved_references
            .retain(|entry| entry.source_kind != "history");
        self.save()
    }
    /// Restores the durable pre-migration profile only when explicitly requested.
    pub fn rollback_migration(&mut self) -> Result<bool, ProfileError> {
        let backup = self.path.with_extension("v1.pre-migration.json");
        if !backup.exists() {
            return Ok(false);
        }
        let restored = serde_json::from_slice::<LocalProfile>(&fs::read(&backup)?)?;
        if restored.schema_version != 1 {
            return Err(ProfileError::UnsupportedSchema(restored.schema_version));
        }
        atomic_write(&self.path, &serde_json::to_vec_pretty(&restored)?)?;
        self.profile = restored;
        Ok(true)
    }
    fn migrate_and_retain(&mut self) -> Result<(), ProfileError> {
        let before = self.profile.clone();
        self.resolve_records();
        self.retain()?;
        if self.profile != before {
            if self.path.exists() {
                fs::copy(
                    &self.path,
                    self.path.with_extension("v1.pre-migration.json"),
                )?;
            }
            self.save()?;
            let journal = serde_json::json!({"sourceSchemaVersion":1,"targetSchemaVersion":1,"timestamp":now()?,"catalogVersion":self.resolver.catalog_version,"migration":"lifecycle-resolution","favourites":self.profile.favourites.len(),"history":self.profile.playback_history.len(),"unresolved":self.profile.unresolved_references.len()});
            atomic_write(
                &self.path.with_extension("v1.migration-journal.json"),
                &serde_json::to_vec_pretty(&journal)?,
            )?;
        }
        Ok(())
    }
    fn resolve_records(&mut self) {
        let mut unresolved = std::mem::take(&mut self.profile.unresolved_references);
        let mut favourites = Vec::new();
        for mut value in std::mem::take(&mut self.profile.favourites) {
            match self.resolver.resolve(&value.station_id) {
                Ok(id) => {
                    value.station_id = id;
                    favourites.push(value)
                }
                Err((reason, candidates)) => unresolved.push(unresolved_for(
                    "favourite",
                    value.station_id,
                    value.added_at,
                    reason,
                    candidates,
                    value.metadata.last_known_name,
                    self.resolver.catalog_version.clone(),
                )),
            }
        }
        favourites.sort_by_key(|f| (f.added_at.clone(), f.station_id.clone()));
        let mut merged = Vec::<Favourite>::new();
        for value in favourites {
            if let Some(existing) = merged
                .iter_mut()
                .find(|item| item.station_id == value.station_id)
            {
                if value.updated_at > existing.updated_at {
                    existing.updated_at = value.updated_at;
                    existing.metadata = value.metadata;
                } else {
                    if existing.metadata.last_known_name.is_none() {
                        existing.metadata.last_known_name = value.metadata.last_known_name;
                    }
                    if existing.metadata.catalog_version.is_none() {
                        existing.metadata.catalog_version = value.metadata.catalog_version;
                    }
                }
            } else {
                merged.push(value);
            }
        }
        self.profile.favourites = merged;
        let mut history = Vec::new();
        for mut value in std::mem::take(&mut self.profile.playback_history) {
            match self.resolver.resolve(&value.station_id) {
                Ok(id) => {
                    value.station_id = id;
                    history.push(value)
                }
                Err((reason, candidates)) => unresolved.push(unresolved_for(
                    "history",
                    value.station_id,
                    value.started_at,
                    reason,
                    candidates,
                    value.metadata.last_known_name,
                    self.resolver.catalog_version.clone(),
                )),
            }
        }
        self.profile.playback_history = history;
        self.profile.unresolved_references = unresolved;
    }
    fn retain(&mut self) -> Result<(), ProfileError> {
        let cutoff = OffsetDateTime::now_utc() - time::Duration::days(HISTORY_RETENTION_DAYS);
        self.profile
            .playback_history
            .retain(|e| parse_time(&e.last_played_at).is_ok_and(|t| t >= cutoff));
        self.profile.playback_history.sort_by(|a, b| {
            b.last_played_at
                .cmp(&a.last_played_at)
                .then_with(|| b.started_at.cmp(&a.started_at))
                .then_with(|| a.record_id.cmp(&b.record_id))
        });
        self.profile.playback_history.truncate(MAX_HISTORY);
        Ok(())
    }
    fn save(&mut self) -> Result<(), ProfileError> {
        self.profile.updated_at = now()?;
        atomic_write(&self.path, &serde_json::to_vec_pretty(&self.profile)?)
    }
}
impl LocalProfile {
    fn new(timestamp: String) -> Self {
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
fn display(station: &Station, catalog_version: Option<String>) -> DisplayMetadata {
    DisplayMetadata {
        last_known_name: Some(station.name.clone()),
        catalog_version,
    }
}
fn unresolved_for(
    source_kind: &str,
    original_station_id: String,
    first_seen_at: String,
    reason: String,
    candidate_station_ids: Vec<String>,
    last_known_name: Option<String>,
    catalog_version: Option<String>,
) -> UnresolvedStationReference {
    UnresolvedStationReference {
        reference_id: Uuid::new_v4(),
        source_kind: source_kind.into(),
        original_station_id,
        first_seen_at,
        reason,
        candidate_station_ids,
        last_known_name,
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
fn now() -> Result<String, ProfileError> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| ProfileError::Invalid(e.to_string()))
}
fn parse_time(value: &str) -> Result<OffsetDateTime, ProfileError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|e| ProfileError::Invalid(e.to_string()))
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ProfileError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?
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
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn station(id: &str, url: &str) -> Station {
        Station::from_primary(
            id.into(),
            id.into(),
            url.into(),
            "".into(),
            "".into(),
            128,
            "mp3".into(),
        )
    }
    fn store(stations: Vec<Station>) -> (PersonalDataStore, PathBuf) {
        let path = std::env::temp_dir().join(format!("rockcast-profile-{}.json", Uuid::new_v4()));
        let resolver = CatalogResolver::from_stations(&stations, Some("test".into()));
        (
            PersonalDataStore::open(path.clone(), resolver).unwrap(),
            path,
        )
    }
    #[test]
    fn favourite_persists_and_uses_id_not_url() {
        let s = station("stable", "https://old");
        let (mut p, path) = store(vec![s.clone()]);
        assert!(p.toggle_favourite(&s).unwrap());
        drop(p);
        let changed = station("stable", "https://new");
        let p = PersonalDataStore::open(
            path.clone(),
            CatalogResolver::from_stations(&[changed], None),
        )
        .unwrap();
        assert!(p.is_favourite("stable"));
        assert_eq!(p.favourites()[0].station_id, "stable");
        let _ = fs::remove_file(path);
    }
    #[test]
    fn stable_id_resolves_to_current_catalog_station() {
        let stations = vec![
            station("first", "https://first"),
            station("stable", "https://changed-stream"),
        ];
        assert_eq!(station_index_by_id(&stations, "stable"), Some(1));
    }

    #[test]
    fn missing_stable_id_is_safe_and_does_not_select_another_station() {
        let stations = vec![station("available", "https://available")];
        assert_eq!(station_index_by_id(&stations, "missing"), None);
    }
    #[test]
    fn history_coalesces_orders_and_clears() {
        let a = station("a", "https://a");
        let b = station("b", "https://b");
        let (mut p, path) = store(vec![a.clone(), b.clone()]);
        p.record_play(&a).unwrap();
        p.record_play(&a).unwrap();
        p.record_play(&b).unwrap();
        assert_eq!(p.history().len(), 2);
        assert_eq!(p.history()[0].station_id, "b");
        p.profile
            .unresolved_references
            .push(UnresolvedStationReference {
                reference_id: Uuid::new_v4(),
                source_kind: "history".into(),
                original_station_id: "missing".into(),
                first_seen_at: now().unwrap(),
                reason: "missing".into(),
                candidate_station_ids: vec![],
                last_known_name: Some("Missing station".into()),
                catalog_version: None,
            });
        p.clear_history().unwrap();
        assert!(p.history().is_empty());
        assert!(p.profile().unresolved_references.is_empty());
        let _ = fs::remove_file(path);
    }
    #[test]
    fn merge_retire_split_and_missing_quarantine() {
        let active = station("new", "https://n");
        let resolver = CatalogResolver::from_stations(&[active], None).with_tombstones(vec![
            Tombstone {
                id: "old".into(),
                reason: TombstoneReason::Merged,
                replacement_ids: vec!["new".into()],
            },
            Tombstone {
                id: "gone".into(),
                reason: TombstoneReason::Removed,
                replacement_ids: vec![],
            },
            Tombstone {
                id: "fork".into(),
                reason: TombstoneReason::Split,
                replacement_ids: vec!["new".into(), "other".into()],
            },
        ]);
        assert_eq!(resolver.resolve("old").unwrap(), "new");
        assert_eq!(resolver.resolve("gone").unwrap_err().0, "removed");
        assert_eq!(resolver.resolve("fork").unwrap_err().0, "split");
        assert_eq!(resolver.resolve("unknown").unwrap_err().0, "missing");
    }
    #[test]
    fn migration_quarantines_legacy_id_without_deleting_it() {
        let path = std::env::temp_dir().join(format!("rockcast-profile-{}.json", Uuid::new_v4()));
        let timestamp = now().unwrap();
        let profile = LocalProfile {
            schema_version: 1,
            profile_id: Uuid::new_v4(),
            created_at: timestamp.clone(),
            updated_at: timestamp.clone(),
            favourites: vec![Favourite {
                record_id: Uuid::new_v4(),
                station_id: "legacy-deadbeef".into(),
                added_at: timestamp.clone(),
                updated_at: timestamp,
                metadata: DisplayMetadata::default(),
            }],
            playback_history: vec![],
            unresolved_references: vec![],
            metadata: Default::default(),
        };
        fs::write(&path, serde_json::to_vec(&profile).unwrap()).unwrap();
        let store =
            PersonalDataStore::open(path.clone(), CatalogResolver::from_stations(&[], None))
                .unwrap();
        assert!(store.favourites().is_empty());
        assert_eq!(
            store.profile().unresolved_references[0].reason,
            "legacy-unmapped"
        );
        assert_eq!(
            store.profile().unresolved_references[0].original_station_id,
            "legacy-deadbeef"
        );
        assert!(path.with_extension("v1.pre-migration.json").exists());
        let _ = fs::remove_file(path);
    }
    #[test]
    fn retention_removes_old_entries() {
        let a = station("a", "https://a");
        let (mut store, path) = store(vec![a]);
        let old = (OffsetDateTime::now_utc() - time::Duration::days(91))
            .format(&Rfc3339)
            .unwrap();
        store.profile.playback_history.push(PlaybackHistoryEntry {
            record_id: Uuid::new_v4(),
            station_id: "a".into(),
            started_at: old.clone(),
            last_played_at: old,
            ended_at: None,
            play_duration_ms: None,
            metadata: HistoryMetadata::default(),
        });
        store.retain().unwrap();
        assert!(store.history().is_empty());
        let _ = fs::remove_file(path);
    }
}
