//! Per-device sync state: the last applied `server_revision` cursor plus the
//! exact record versions the server has acknowledged. Diffing the live profile
//! against this base yields the next push batch.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::personal_data::{
    Favourite, LocalProfile, PlaybackHistoryEntry, atomic_write, parse_time,
};

use super::contract::{
    FavouriteChanges, FavouritePush, HistoryChanges, HistoryPush, RecordDelete, SyncRequest,
};

/// Server-side batch cap per collection (OpenAPI `max_batch_items_per_collection`).
pub(crate) const BATCH_LIMIT: usize = 300;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub server_revision: u64,
    #[serde(default)]
    pub favourites: HashMap<Uuid, Favourite>,
    #[serde(default)]
    pub history: HashMap<Uuid, PlaybackHistoryEntry>,
}

pub(crate) fn state_path() -> PathBuf {
    crate::settings::app_dir()
        .map(|d| d.join("personal-sync-state.v1.json"))
        .unwrap_or_else(|| PathBuf::from("rockcast-personal-sync-state.v1.json"))
}

impl From<Favourite> for FavouritePush {
    fn from(value: Favourite) -> Self {
        Self {
            record_id: value.record_id,
            station_id: value.station_id,
            added_at: value.added_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<PlaybackHistoryEntry> for HistoryPush {
    fn from(value: PlaybackHistoryEntry) -> Self {
        let metadata = serde_json::to_value(&value.metadata)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .filter(|map| !map.is_empty());
        Self {
            record_id: value.record_id,
            station_id: value.station_id,
            started_at: value.started_at,
            last_played_at: value.last_played_at,
            ended_at: value.ended_at,
            play_duration_ms: value.play_duration_ms,
            metadata,
            updated_at: value.updated_at,
        }
    }
}

impl SyncState {
    pub fn fresh() -> Self {
        Self {
            schema_version: 1,
            server_revision: 0,
            favourites: HashMap::new(),
            history: HashMap::new(),
        }
    }
    /// Loads the durable per-device state; a missing, corrupt, or
    /// forward-incompatible file is a fresh state (full snapshot pull), never
    /// a sync failure.
    pub fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<SyncState>(&bytes).ok())
            .filter(|state| state.schema_version <= 1)
            .unwrap_or_else(Self::fresh)
    }
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        atomic_write(path, &serde_json::to_vec_pretty(self)?)
            .map_err(|error| std::io::Error::other(error.to_string()))
    }
    /// Base state capturing "the server has seen exactly this profile". Used
    /// after a fully applied sync cycle, so the next diff contains only real
    /// local changes.
    pub fn matched_to(profile: &LocalProfile) -> Self {
        Self {
            favourites: profile
                .favourites
                .iter()
                .cloned()
                .map(|favourite| (favourite.record_id, favourite))
                .collect(),
            history: profile
                .playback_history
                .iter()
                .cloned()
                .map(|entry| (entry.record_id, entry))
                .collect(),
            ..Self::fresh()
        }
    }
    /// Local changes the server has not acknowledged yet.
    pub fn diff(&self, profile: &LocalProfile) -> (FavouriteChanges, HistoryChanges) {
        let mut favourites = FavouriteChanges::default();
        for favourite in &profile.favourites {
            let unchanged = self
                .favourites
                .get(&favourite.record_id)
                .is_some_and(|base| base == favourite);
            if !unchanged {
                favourites.upserts.push(favourite.clone().into());
            }
        }
        for (record_id, base) in &self.favourites {
            if !profile.favourites.iter().any(|f| f.record_id == *record_id) {
                favourites.deletes.push(RecordDelete {
                    record_id: *record_id,
                    updated_at: tombstone_updated_at(&base.updated_at),
                });
            }
        }
        let mut history = HistoryChanges::default();
        for entry in &profile.playback_history {
            let unchanged = self
                .history
                .get(&entry.record_id)
                .is_some_and(|base| base == entry);
            if !unchanged {
                history.upserts.push(entry.clone().into());
            }
        }
        for (record_id, base) in &self.history {
            if !profile
                .playback_history
                .iter()
                .any(|e| e.record_id == *record_id)
            {
                history.deletes.push(RecordDelete {
                    record_id: *record_id,
                    updated_at: tombstone_updated_at(&base.updated_at),
                });
            }
        }
        (favourites, history)
    }
}

/// A tombstone must beat the newest version this device ever observed for the
/// record — including versions authored by other devices' clocks — so it is
/// stamped one instant after `base.updated_at`, or now, whichever is later.
fn tombstone_updated_at(base: &str) -> String {
    let now = OffsetDateTime::now_utc();
    let newer_than_base = parse_time(base)
        .map(|at| at + Duration::nanoseconds(1))
        .unwrap_or(now);
    let stamp = newer_than_base.max(now);
    stamp
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| now.to_string())
}

/// Splits one pending push into server-legal requests of at most 300 upserts
/// and 300 deletes per collection. `since_revision` rides only on the first
/// request; later requests carry the cursor returned by the previous chunk.
pub(crate) fn chunk_requests(
    favourites: FavouriteChanges,
    history: HistoryChanges,
    since_revision: u64,
) -> Vec<SyncRequest> {
    let favourite_parts = collection_chunks(favourites.upserts, favourites.deletes);
    let history_parts = collection_chunks(history.upserts, history.deletes);
    let chunks = favourite_parts.len().max(history_parts.len());
    (0..chunks)
        .map(|index| {
            let favourites = favourite_parts.get(index).and_then(|(upserts, deletes)| {
                (!upserts.is_empty() || !deletes.is_empty()).then(|| FavouriteChanges {
                    upserts: upserts.clone(),
                    deletes: deletes.clone(),
                })
            });
            let history = history_parts.get(index).and_then(|(upserts, deletes)| {
                (!upserts.is_empty() || !deletes.is_empty()).then(|| HistoryChanges {
                    upserts: upserts.clone(),
                    deletes: deletes.clone(),
                })
            });
            SyncRequest {
                since_revision: (index == 0 && since_revision > 0).then_some(since_revision),
                favourites,
                history,
            }
        })
        .collect()
}

fn collection_chunks<U: Clone>(
    upserts: Vec<U>,
    deletes: Vec<RecordDelete>,
) -> Vec<(Vec<U>, Vec<RecordDelete>)> {
    if upserts.is_empty() && deletes.is_empty() {
        return Vec::new();
    }
    let upsert_chunks: Vec<Vec<U>> = upserts.chunks(BATCH_LIMIT).map(|c| c.to_vec()).collect();
    let delete_chunks: Vec<Vec<RecordDelete>> =
        deletes.chunks(BATCH_LIMIT).map(|c| c.to_vec()).collect();
    (0..upsert_chunks.len().max(delete_chunks.len()))
        .map(|index| {
            (
                upsert_chunks.get(index).cloned().unwrap_or_default(),
                delete_chunks.get(index).cloned().unwrap_or_default(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::personal_data::HistoryMetadata;

    fn favourite(record_id: Uuid, station: &str, updated_at: &str) -> Favourite {
        Favourite {
            record_id,
            station_id: station.into(),
            added_at: "2026-09-01T00:00:00Z".into(),
            updated_at: updated_at.into(),
            metadata: Default::default(),
        }
    }

    fn profile(favourites: Vec<Favourite>, history: Vec<PlaybackHistoryEntry>) -> LocalProfile {
        LocalProfile {
            schema_version: 1,
            profile_id: Uuid::new_v4(),
            created_at: "2026-09-01T00:00:00Z".into(),
            updated_at: "2026-09-01T00:00:00Z".into(),
            favourites,
            playback_history: history,
            unresolved_references: vec![],
            metadata: Default::default(),
        }
    }

    #[test]
    fn fresh_state_pulls_full_snapshot_and_pushes_everything() {
        let state = SyncState::fresh();
        let live = profile(
            vec![favourite(Uuid::new_v4(), "a", "2026-09-01T00:00:00Z")],
            vec![],
        );
        let (favourites, history) = state.diff(&live);
        assert_eq!(favourites.upserts.len(), 1);
        assert!(favourites.deletes.is_empty());
        assert!(history.upserts.is_empty() && history.deletes.is_empty());
        let requests = chunk_requests(favourites, history, 0);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].since_revision, None);
        assert!(requests[0].history.is_none());
    }

    #[test]
    fn diff_reports_only_changed_and_deleted_records() {
        let kept = Uuid::new_v4();
        let changed = Uuid::new_v4();
        let removed = Uuid::new_v4();
        let mut state = SyncState::fresh();
        state
            .favourites
            .insert(kept, favourite(kept, "a", "2026-09-01T00:00:00Z"));
        state
            .favourites
            .insert(changed, favourite(changed, "b", "2026-09-01T00:00:00Z"));
        state
            .favourites
            .insert(removed, favourite(removed, "c", "2026-09-01T00:00:00Z"));
        let live = profile(
            vec![
                favourite(kept, "a", "2026-09-01T00:00:00Z"),
                favourite(changed, "b", "2026-09-05T00:00:00Z"),
            ],
            vec![],
        );
        let (favourites, _) = state.diff(&live);
        assert_eq!(favourites.upserts.len(), 1);
        assert_eq!(favourites.upserts[0].record_id, changed);
        assert_eq!(favourites.deletes.len(), 1);
        assert_eq!(favourites.deletes[0].record_id, removed);
        assert!(newer_than(
            &favourites.deletes[0].updated_at,
            "2026-09-01T00:00:00Z"
        ));
    }

    #[test]
    fn tombstone_beats_a_future_dated_base_version() {
        let stamp = tombstone_updated_at("2999-01-01T00:00:00Z");
        assert!(newer_than(&stamp, "2999-01-01T00:00:00Z"));
    }

    fn newer_than(left: &str, right: &str) -> bool {
        crate::personal_data::parse_time(left).unwrap()
            > crate::personal_data::parse_time(right).unwrap()
    }

    #[test]
    fn oversized_push_is_split_into_legal_chunks() {
        let upserts = (0..=BATCH_LIMIT)
            .map(|_| FavouritePush {
                record_id: Uuid::new_v4(),
                station_id: "a".into(),
                added_at: "2026-09-01T00:00:00Z".into(),
                updated_at: "2026-09-01T00:00:00Z".into(),
            })
            .collect();
        let deletes = (0..(BATCH_LIMIT * 2))
            .map(|_| RecordDelete {
                record_id: Uuid::new_v4(),
                updated_at: "2026-09-01T00:00:00Z".into(),
            })
            .collect();
        let requests = chunk_requests(
            FavouriteChanges { upserts, deletes },
            HistoryChanges::default(),
            42,
        );
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].since_revision, Some(42));
        assert!(requests[1].since_revision.is_none());
        let upserts: usize = requests
            .iter()
            .map(|r| r.favourites.as_ref().map_or(0, |f| f.upserts.len()))
            .sum();
        let deletes: usize = requests
            .iter()
            .map(|r| r.favourites.as_ref().map_or(0, |f| f.deletes.len()))
            .sum();
        assert_eq!(upserts, BATCH_LIMIT + 1);
        assert_eq!(deletes, BATCH_LIMIT * 2);
        for request in &requests {
            let favourites = request.favourites.as_ref().unwrap();
            assert!(favourites.upserts.len() <= BATCH_LIMIT);
            assert!(favourites.deletes.len() <= BATCH_LIMIT);
        }
    }

    #[test]
    fn state_roundtrips_through_disk() {
        let path =
            std::env::temp_dir().join(format!("rockcast-sync-state-{}.json", Uuid::new_v4()));
        let mut state = SyncState::fresh();
        state.server_revision = 17;
        let id = Uuid::new_v4();
        state
            .favourites
            .insert(id, favourite(id, "a", "2026-09-01T00:00:00Z"));
        state.save(&path).unwrap();
        let loaded = SyncState::load(&path);
        assert_eq!(loaded, state);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn corrupt_state_file_is_a_fresh_state() {
        let path =
            std::env::temp_dir().join(format!("rockcast-sync-state-{}.json", Uuid::new_v4()));
        fs::write(&path, b"{not json").unwrap();
        assert_eq!(SyncState::load(&path), SyncState::fresh());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn history_push_carries_metadata_object() {
        let entry = PlaybackHistoryEntry {
            record_id: Uuid::new_v4(),
            station_id: "a".into(),
            started_at: "2026-09-05T10:00:00Z".into(),
            last_played_at: "2026-09-05T10:05:00Z".into(),
            updated_at: "2026-09-05T10:05:00Z".into(),
            ended_at: None,
            play_duration_ms: Some(1_000),
            metadata: HistoryMetadata {
                last_known_name: Some("A".into()),
                catalog_version: None,
                source: Some("bundled".into()),
                extra: Default::default(),
            },
        };
        let push = HistoryPush::from(entry);
        let metadata = push.metadata.unwrap();
        assert_eq!(metadata.get("lastKnownName"), Some(&serde_json::json!("A")));
        assert_eq!(push.play_duration_ms, Some(1_000));
    }
}
