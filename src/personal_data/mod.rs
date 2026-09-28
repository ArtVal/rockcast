//! Offline-only favourites and playback history (RM-007-A profile v1).
//! Sync application lives in [`sync_apply`]; wire types live in
//! [`crate::personal_sync`].

mod records;
mod sync_apply;

#[cfg(test)]
mod tests;

pub use records::{
    CatalogResolver, DisplayMetadata, Favourite, HistoryMetadata, LocalProfile,
    PlaybackHistoryEntry, ProfileError, Tombstone, TombstoneReason, UnresolvedStationReference,
};
pub(crate) use records::{atomic_write, now, parse_time, station_index_by_id};

use std::{collections::HashMap, path::PathBuf};

use records::display;
use uuid::Uuid;

const MAX_FAVOURITES: usize = 500;
const MAX_HISTORY: usize = 500;
const HISTORY_RETENTION_DAYS: i64 = 90;
const COALESCE_SECONDS: i64 = 5 * 60;

pub struct PersonalDataStore {
    path: PathBuf,
    profile: LocalProfile,
    resolver: CatalogResolver,
    /// True when no profile file existed and a blank one was created. The sync
    /// layer resets its server cursor state in that case so a lost profile file
    /// can never masquerade as "user deleted everything" and tombstone the
    /// account (RM-012-B safety).
    created_fresh: bool,
}
impl PersonalDataStore {
    pub fn open(path: PathBuf, resolver: CatalogResolver) -> Result<Self, ProfileError> {
        let created_fresh = !path.exists();
        let profile = if created_fresh {
            LocalProfile::new(now()?)
        } else {
            serde_json::from_slice::<LocalProfile>(&std::fs::read(&path)?)?
        };
        if profile.schema_version != 1 {
            return Err(ProfileError::UnsupportedSchema(profile.schema_version));
        }
        let mut store = Self {
            path,
            profile,
            resolver,
            created_fresh,
        };
        store.backfill_history_updated_at();
        store.migrate_and_retain()?;
        Ok(store)
    }
    pub fn created_fresh(&self) -> bool {
        self.created_fresh
    }
    /// Populates the sync version field for history records written before
    /// RM-012-B; `last_played_at` is the truthful last server-visible write.
    fn backfill_history_updated_at(&mut self) {
        for entry in &mut self.profile.playback_history {
            if entry.updated_at.is_empty() {
                entry.updated_at = entry.last_played_at.clone();
            }
        }
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
    pub fn toggle_favourite(
        &mut self,
        station: &crate::stations::Station,
    ) -> Result<bool, ProfileError> {
        if self.is_favourite(&station.id) {
            self.remove_favourite(&station.id)?;
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
    /// Removes one favourite by station id — used when only the profile
    /// record exists (the station is not in the loaded catalog). Returns
    /// whether the favourite existed.
    pub fn remove_favourite(&mut self, station_id: &str) -> Result<bool, ProfileError> {
        let existed = self.is_favourite(station_id);
        self.profile
            .favourites
            .retain(|f| f.station_id != station_id);
        if existed {
            self.save()?;
        }
        Ok(existed)
    }
    pub fn record_play(&mut self, station: &crate::stations::Station) -> Result<(), ProfileError> {
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
            entry.updated_at = ts.clone();
        } else {
            self.profile.playback_history.push(PlaybackHistoryEntry {
                record_id: Uuid::new_v4(),
                station_id: station.id.clone(),
                started_at: ts.clone(),
                last_played_at: ts.clone(),
                updated_at: ts.clone(),
                ended_at: Some(ts),
                play_duration_ms: Some(0),
                metadata: HistoryMetadata {
                    last_known_name: Some(station.name.clone()),
                    catalog_version: self.resolver.catalog_version.clone(),
                    source: Some("bundled".into()),
                    extra: Default::default(),
                },
            });
        }
        self.retain()?;
        self.save()
    }
    pub fn clear_history(&mut self) -> Result<(), ProfileError> {
        self.profile.playback_history.clear();
        self.save()
    }
    /// Restores the durable pre-migration profile only when explicitly requested.
    pub fn rollback_migration(&mut self) -> Result<bool, ProfileError> {
        let backup = self.path.with_extension("v1.pre-migration.json");
        if !backup.exists() {
            return Ok(false);
        }
        let restored = serde_json::from_slice::<LocalProfile>(&std::fs::read(&backup)?)?;
        if restored.schema_version != 1 {
            return Err(ProfileError::UnsupportedSchema(restored.schema_version));
        }
        atomic_write(&self.path, &serde_json::to_vec_pretty(&restored)?)?;
        self.profile = restored;
        Ok(true)
    }
    fn migrate_and_retain(&mut self) -> Result<(), ProfileError> {
        let before = self.profile.clone();
        self.restore_quarantined_records();
        self.remap_legacy_station_ids();
        self.backfill_favourite_names();
        self.retain()?;
        if self.profile != before {
            if self.path.exists() {
                std::fs::copy(
                    &self.path,
                    self.path.with_extension("v1.pre-migration.json"),
                )?;
            }
            self.save()?;
            let journal = serde_json::json!({"sourceSchemaVersion":1,"targetSchemaVersion":1,"timestamp":now()?,"catalogVersion":self.resolver.catalog_version,"migration":"restore-and-remap","favourites":self.profile.favourites.len(),"history":self.profile.playback_history.len(),"unresolved":self.profile.unresolved_references.len()});
            atomic_write(
                &self.path.with_extension("v1.migration-journal.json"),
                &serde_json::to_vec_pretty(&journal)?,
            )?;
        }
        Ok(())
    }
    /// One-time undo of the RM-007-A quarantine: records previously moved to
    /// `unresolvedReferences` return to the live collections with their
    /// original station ids. Nothing is quarantined anymore — a station id the
    /// current catalog cannot resolve stays in the profile (it is listed, and
    /// simply plays nothing until the station is discoverable again), which
    /// also keeps server sync from mistaking such records for deletions
    /// (RM-012-B: unresolved references are valid).
    fn restore_quarantined_records(&mut self) {
        for entry in std::mem::take(&mut self.profile.unresolved_references) {
            match entry.source_kind.as_str() {
                "favourite" => self.profile.favourites.push(Favourite {
                    record_id: entry.reference_id,
                    station_id: entry.original_station_id,
                    added_at: entry.first_seen_at.clone(),
                    updated_at: entry.first_seen_at,
                    metadata: DisplayMetadata {
                        last_known_name: entry.last_known_name,
                        catalog_version: entry.catalog_version,
                    },
                }),
                _ => self.profile.playback_history.push(PlaybackHistoryEntry {
                    record_id: entry.reference_id,
                    station_id: entry.original_station_id,
                    started_at: entry.first_seen_at.clone(),
                    last_played_at: entry.first_seen_at.clone(),
                    updated_at: entry.first_seen_at,
                    ended_at: None,
                    play_duration_ms: None,
                    metadata: HistoryMetadata {
                        last_known_name: entry.last_known_name,
                        catalog_version: entry.catalog_version,
                        source: None,
                        extra: Default::default(),
                    },
                }),
            }
        }
    }
    /// Favourites applied from server sync carry no display metadata (the
    /// RM-012-A contract has none), so a missing display name is recovered
    /// from the newest history entry of the same station.
    fn backfill_favourite_names(&mut self) {
        let mut newest_name: HashMap<&str, (String, String)> = HashMap::new();
        for entry in &self.profile.playback_history {
            let Some(name) = &entry.metadata.last_known_name else {
                continue;
            };
            let newer = newest_name
                .get(entry.station_id.as_str())
                .is_none_or(|(last_played_at, _)| *last_played_at < entry.last_played_at);
            if newer {
                newest_name.insert(
                    entry.station_id.as_str(),
                    (entry.last_played_at.clone(), name.clone()),
                );
            }
        }
        for favourite in &mut self.profile.favourites {
            if favourite.metadata.last_known_name.is_none()
                && let Some((_, name)) = newest_name.get(favourite.station_id.as_str())
            {
                favourite.metadata.last_known_name = Some(name.clone());
            }
        }
    }

    /// Rewrites legacy station ids to their current catalog ids when the
    /// mapping is known and collapses duplicate favourites by station id.
    /// Unresolvable ids are left untouched.
    fn remap_legacy_station_ids(&mut self) {
        for favourite in &mut self.profile.favourites {
            if let Ok(id) = self.resolver.resolve(&favourite.station_id) {
                favourite.station_id = id;
            }
        }
        for entry in &mut self.profile.playback_history {
            if let Ok(id) = self.resolver.resolve(&entry.station_id) {
                entry.station_id = id;
            }
        }
        self.profile
            .favourites
            .sort_by_key(|f| (f.added_at.clone(), f.station_id.clone()));
        let mut merged = Vec::<Favourite>::new();
        for value in std::mem::take(&mut self.profile.favourites) {
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
    }
    fn retain(&mut self) -> Result<(), ProfileError> {
        let cutoff = time::OffsetDateTime::now_utc() - time::Duration::days(HISTORY_RETENTION_DAYS);
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
