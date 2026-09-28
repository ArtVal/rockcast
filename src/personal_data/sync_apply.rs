//! Applying incoming RM-012-B sync records to the local profile.
//!
//! Deletion, LWW ordering, and collection caps reuse the RM-007-A lifecycle
//! rules; this module only translates wire records into profile writes.

use crate::personal_sync::{FavouriteRecord, HistoryRecord};

use super::{
    Favourite, HistoryMetadata, PersonalDataStore, PlaybackHistoryEntry, ProfileError, parse_time,
};

const MAX_FAVOURITES: usize = 500;

impl PersonalDataStore {
    /// Applies one sync response batch with the existing RM-007-A lifecycle
    /// rules: last-writer-wins per `record_id` on `updated_at` (strictly newer
    /// replaces, ties keep the local record), `deleted_at` removes the record,
    /// and retention runs afterwards so cap evictions become ordinary local
    /// deletions for the next push. Returns how many records changed the
    /// profile.
    pub(crate) fn apply_incoming_records(
        &mut self,
        favourites: &[FavouriteRecord],
        history: &[HistoryRecord],
    ) -> Result<usize, ProfileError> {
        let mut applied = 0;
        for record in favourites {
            if record.deleted_at.is_some() {
                let before = self.profile.favourites.len();
                self.profile
                    .favourites
                    .retain(|f| f.record_id != record.record_id);
                applied += usize::from(before != self.profile.favourites.len());
                continue;
            }
            let Some(station_id) = record.station_id.as_deref() else {
                continue;
            };
            let Ok(incoming) = parse_time(&record.updated_at) else {
                continue;
            };
            let existing = self
                .profile
                .favourites
                .iter_mut()
                .find(|f| f.record_id == record.record_id);
            if let Some(local) = existing {
                if parse_time(&local.updated_at).is_ok_and(|local| incoming > local) {
                    // Favourite display metadata is a local presentation concern
                    // and survives replacement because the server contract
                    // carries no favourite metadata.
                    let metadata = local.metadata.clone();
                    *local = Favourite {
                        record_id: record.record_id,
                        station_id: station_id.to_owned(),
                        added_at: record.added_at.clone(),
                        updated_at: record.updated_at.clone(),
                        metadata,
                    };
                    applied += 1;
                }
            } else {
                self.profile.favourites.push(Favourite {
                    record_id: record.record_id,
                    station_id: station_id.to_owned(),
                    added_at: record.added_at.clone(),
                    updated_at: record.updated_at.clone(),
                    metadata: Default::default(),
                });
                applied += 1;
            }
        }
        if self.profile.favourites.len() > MAX_FAVOURITES {
            self.profile.favourites.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then(a.record_id.cmp(&b.record_id))
            });
            self.profile.favourites.truncate(MAX_FAVOURITES);
        }
        for record in history {
            if record.deleted_at.is_some() {
                let before = self.profile.playback_history.len();
                self.profile
                    .playback_history
                    .retain(|e| e.record_id != record.record_id);
                applied += usize::from(before != self.profile.playback_history.len());
                continue;
            }
            let Some(station_id) = record.station_id.as_deref() else {
                continue;
            };
            let Ok(incoming) = parse_time(&record.updated_at) else {
                continue;
            };
            let metadata = record
                .metadata
                .as_ref()
                .map(|map| {
                    serde_json::from_value::<HistoryMetadata>(serde_json::Value::Object(
                        map.clone(),
                    ))
                    .unwrap_or_default()
                })
                .unwrap_or_default();
            let incoming_entry = PlaybackHistoryEntry {
                record_id: record.record_id,
                station_id: station_id.to_owned(),
                started_at: record.started_at.clone(),
                last_played_at: record.last_played_at.clone(),
                updated_at: record.updated_at.clone(),
                ended_at: record.ended_at.clone(),
                play_duration_ms: record.play_duration_ms,
                metadata,
            };
            let existing = self
                .profile
                .playback_history
                .iter_mut()
                .find(|e| e.record_id == record.record_id);
            match existing {
                Some(local)
                    if parse_time(&local.updated_at).is_ok_and(|local| incoming > local) =>
                {
                    *local = incoming_entry;
                    applied += 1;
                }
                Some(_) => {}
                None => {
                    self.profile.playback_history.push(incoming_entry);
                    applied += 1;
                }
            }
        }
        self.backfill_favourite_names();
        self.retain()?;
        self.save()?;
        Ok(applied)
    }
}
