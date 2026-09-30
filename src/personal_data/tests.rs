use super::*;
use std::{fs, path::PathBuf};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

fn station(id: &str, url: &str) -> crate::stations::Station {
    crate::stations::Station::from_primary(
        id.into(),
        id.into(),
        url.into(),
        "".into(),
        "".into(),
        128,
        "mp3".into(),
    )
}
fn store(stations: Vec<crate::stations::Station>) -> (PersonalDataStore, PathBuf) {
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
fn remove_favourite_works_without_a_catalog_station() {
    let a = station("a", "https://a");
    let (mut p, path) = store(vec![a.clone()]);
    assert!(p.toggle_favourite(&a).unwrap());
    assert!(p.remove_favourite("a").unwrap());
    assert!(!p.is_favourite("a"));
    assert!(!p.remove_favourite("a").unwrap());
    let _ = fs::remove_file(path);
}

#[test]
fn toggle_favourite_by_id_works_without_catalog_station() {
    let (mut p, path) = store(vec![]);
    assert!(p.toggle_favourite_by_id("non-catalog-st", "Non Catalog Station").unwrap());
    assert!(p.is_favourite("non-catalog-st"));
    assert_eq!(
        p.favourites()[0].metadata.last_known_name.as_deref(),
        Some("Non Catalog Station")
    );
    assert!(!p.toggle_favourite_by_id("non-catalog-st", "Non Catalog Station").unwrap());
    assert!(!p.is_favourite("non-catalog-st"));
    assert!(p.favourites().is_empty());
    let _ = fs::remove_file(path);
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
    p.clear_history().unwrap();
    assert!(p.history().is_empty());
    let _ = fs::remove_file(path);
}
#[test]
fn resolver_maps_merges_and_reports_missing() {
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
fn unresolvable_favourite_survives_open_untouched() {
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
            updated_at: timestamp.clone(),
            metadata: DisplayMetadata {
                last_known_name: Some("Old friend".into()),
                catalog_version: None,
            },
        }],
        playback_history: vec![],
        unresolved_references: vec![],
        metadata: Default::default(),
    };
    fs::write(&path, serde_json::to_vec(&profile).unwrap()).unwrap();
    let store =
        PersonalDataStore::open(path.clone(), CatalogResolver::from_stations(&[], None)).unwrap();
    // The RM-007-A quarantine is retired: an id the current catalog cannot
    // resolve keeps its record live instead of being hidden.
    assert_eq!(store.favourites().len(), 1);
    assert_eq!(store.favourites()[0].station_id, "legacy-deadbeef");
    assert_eq!(
        store.favourites()[0].metadata.last_known_name,
        Some("Old friend".into())
    );
    assert!(store.profile().unresolved_references.is_empty());
    let _ = fs::remove_file(path);
}
#[test]
fn favourite_name_backfills_from_history_on_open() {
    let path = std::env::temp_dir().join(format!("rockcast-profile-{}.json", Uuid::new_v4()));
    let timestamp = now().unwrap();
    let profile = LocalProfile {
        schema_version: 1,
        profile_id: Uuid::new_v4(),
        created_at: timestamp.clone(),
        updated_at: timestamp,
        favourites: vec![Favourite {
            record_id: Uuid::new_v4(),
            station_id: "rb-x".into(),
            added_at: "2026-09-20T10:00:00Z".into(),
            updated_at: "2026-09-20T10:00:00Z".into(),
            metadata: DisplayMetadata::default(),
        }],
        playback_history: vec![
            PlaybackHistoryEntry {
                record_id: Uuid::new_v4(),
                station_id: "rb-x".into(),
                started_at: "2026-09-25T10:00:00Z".into(),
                last_played_at: "2026-09-25T10:05:00Z".into(),
                updated_at: "2026-09-25T10:05:00Z".into(),
                ended_at: None,
                play_duration_ms: None,
                metadata: HistoryMetadata {
                    last_known_name: Some("RadioBOB Deutsch Rock".into()),
                    ..HistoryMetadata::default()
                },
            },
            PlaybackHistoryEntry {
                record_id: Uuid::new_v4(),
                station_id: "rb-x".into(),
                started_at: "2026-09-26T10:00:00Z".into(),
                last_played_at: "2026-09-26T10:05:00Z".into(),
                updated_at: "2026-09-26T10:05:00Z".into(),
                ended_at: None,
                play_duration_ms: None,
                metadata: HistoryMetadata {
                    last_known_name: Some("RadioBOB (newer)".into()),
                    ..HistoryMetadata::default()
                },
            },
        ],
        unresolved_references: vec![],
        metadata: Default::default(),
    };
    fs::write(&path, serde_json::to_vec(&profile).unwrap()).unwrap();
    let store = PersonalDataStore::open(
        path.clone(),
        CatalogResolver::from_stations(&[station("rb-x", "https://x")], None),
    )
    .unwrap();
    assert_eq!(
        store.favourites()[0].metadata.last_known_name,
        Some("RadioBOB (newer)".into())
    );
    let _ = fs::remove_file(path);
}

#[test]
fn quarantined_records_are_restored_on_open() {
    let path = std::env::temp_dir().join(format!("rockcast-profile-{}.json", Uuid::new_v4()));
    let timestamp = now().unwrap();
    let profile = LocalProfile {
        schema_version: 1,
        profile_id: Uuid::new_v4(),
        created_at: timestamp.clone(),
        updated_at: timestamp.clone(),
        favourites: vec![],
        playback_history: vec![],
        unresolved_references: vec![
            UnresolvedStationReference {
                reference_id: Uuid::new_v4(),
                source_kind: "favourite".into(),
                original_station_id: "radio-browser-15008023472828250746".into(),
                first_seen_at: "2026-09-20T10:00:00Z".into(),
                reason: "missing".into(),
                candidate_station_ids: vec![],
                last_known_name: Some("Exclusive Radio – Black Sabbath".into()),
                catalog_version: None,
            },
            UnresolvedStationReference {
                reference_id: Uuid::new_v4(),
                source_kind: "history".into(),
                original_station_id: "rb-84c03be7-1197-4a6a-9705-9abbbdba1b28".into(),
                first_seen_at: timestamp.clone(),
                reason: "missing".into(),
                candidate_station_ids: vec![],
                last_known_name: Some("Авторадио".into()),
                catalog_version: None,
            },
        ],
        metadata: Default::default(),
    };
    fs::write(&path, serde_json::to_vec(&profile).unwrap()).unwrap();
    let store =
        PersonalDataStore::open(path.clone(), CatalogResolver::from_stations(&[], None)).unwrap();
    assert_eq!(store.favourites().len(), 1);
    assert_eq!(
        store.favourites()[0].station_id,
        "radio-browser-15008023472828250746"
    );
    assert_eq!(
        store.favourites()[0].metadata.last_known_name,
        Some("Exclusive Radio – Black Sabbath".into())
    );
    assert_eq!(store.history().len(), 1);
    assert_eq!(
        store.history()[0].station_id,
        "rb-84c03be7-1197-4a6a-9705-9abbbdba1b28"
    );
    assert_eq!(
        store.history()[0].metadata.last_known_name,
        Some("Авторадио".into())
    );
    assert!(store.profile().unresolved_references.is_empty());
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
        last_played_at: old.clone(),
        updated_at: old,
        ended_at: None,
        play_duration_ms: None,
        metadata: HistoryMetadata::default(),
    });
    store.retain().unwrap();
    assert!(store.history().is_empty());
    let _ = fs::remove_file(path);
}
#[test]
fn fresh_profile_backfills_history_updated_at_from_last_played() {
    let path = std::env::temp_dir().join(format!("rockcast-profile-{}.json", Uuid::new_v4()));
    let timestamp = now().unwrap();
    let profile = LocalProfile {
        schema_version: 1,
        profile_id: Uuid::new_v4(),
        created_at: timestamp.clone(),
        updated_at: timestamp,
        favourites: vec![],
        playback_history: vec![PlaybackHistoryEntry {
            record_id: Uuid::new_v4(),
            station_id: "a".into(),
            started_at: "2026-09-01T10:00:00Z".into(),
            last_played_at: "2026-09-01T10:05:00Z".into(),
            updated_at: String::new(),
            ended_at: None,
            play_duration_ms: None,
            metadata: HistoryMetadata::default(),
        }],
        unresolved_references: vec![],
        metadata: Default::default(),
    };
    fs::write(&path, serde_json::to_vec(&profile).unwrap()).unwrap();
    let store = PersonalDataStore::open(
        path.clone(),
        CatalogResolver::from_stations(&[station("a", "https://a")], None),
    )
    .unwrap();
    assert!(!store.created_fresh());
    assert_eq!(store.history()[0].updated_at, "2026-09-01T10:05:00Z");
    let _ = fs::remove_file(path);
}

mod apply {
    use super::*;
    use crate::personal_sync::{FavouriteRecord, HistoryRecord};

    fn fav(record_id: Uuid, station: &str, updated_at: &str) -> FavouriteRecord {
        FavouriteRecord {
            record_id,
            station_id: Some(station.into()),
            added_at: "2026-09-01T00:00:00Z".into(),
            updated_at: updated_at.into(),
            deleted_at: None,
        }
    }
    fn hist(record_id: Uuid, station: &str, updated_at: &str) -> HistoryRecord {
        HistoryRecord {
            record_id,
            station_id: Some(station.into()),
            started_at: "2026-09-01T10:00:00Z".into(),
            last_played_at: "2026-09-01T10:05:00Z".into(),
            updated_at: updated_at.into(),
            ended_at: None,
            play_duration_ms: Some(60_000),
            metadata: None,
            deleted_at: None,
        }
    }
    #[test]
    fn incoming_records_apply_by_lww() {
        let a = station("a", "https://a");
        let b = station("b", "https://b");
        let (mut store, path) = store(vec![a, b]);
        let kept = Uuid::new_v4();
        store.profile.favourites.push(Favourite {
            record_id: kept,
            station_id: "a".into(),
            added_at: "2026-09-01T00:00:00Z".into(),
            updated_at: "2026-09-20T00:00:00Z".into(),
            metadata: DisplayMetadata::default(),
        });
        let older_echo = fav(kept, "a", "2026-09-01T00:00:00Z");
        let newer = fav(Uuid::new_v4(), "b", "2026-09-25T00:00:00Z");
        let tie = fav(Uuid::new_v4(), "a", "2026-09-25T00:00:00Z");
        let applied = store
            .apply_incoming_records(&[older_echo, newer, tie], &[])
            .unwrap();
        // Older echo loses to the local write; the new record applies; the tie
        // against a missing local record is a plain insert.
        assert_eq!(applied, 2);
        assert!(store.is_favourite("a"));
        assert!(store.is_favourite("b"));
        assert_eq!(store.favourites().len(), 3);
        let _ = fs::remove_file(path);
    }
    #[test]
    fn equal_timestamp_keeps_local_record() {
        let a = station("a", "https://a");
        let (mut store, path) = store(vec![a]);
        let id = Uuid::new_v4();
        store.profile.favourites.push(Favourite {
            record_id: id,
            station_id: "a".into(),
            added_at: "2026-09-01T00:00:00Z".into(),
            updated_at: "2026-09-20T00:00:00Z".into(),
            metadata: DisplayMetadata {
                last_known_name: Some("Local name".into()),
                catalog_version: None,
            },
        });
        let echo = fav(id, "a", "2026-09-20T00:00:00Z");
        store.apply_incoming_records(&[echo], &[]).unwrap();
        assert_eq!(
            store.favourites()[0].metadata.last_known_name,
            Some("Local name".into())
        );
        let _ = fs::remove_file(path);
    }
    #[test]
    fn tombstones_delete_locally() {
        let a = station("a", "https://a");
        let (mut store, path) = store(vec![a]);
        let fav_id = Uuid::new_v4();
        let hist_id = Uuid::new_v4();
        store.profile.favourites.push(Favourite {
            record_id: fav_id,
            station_id: "a".into(),
            added_at: "2026-09-01T00:00:00Z".into(),
            updated_at: "2026-09-01T00:00:00Z".into(),
            metadata: DisplayMetadata::default(),
        });
        store.profile.playback_history.push(PlaybackHistoryEntry {
            record_id: hist_id,
            station_id: "a".into(),
            started_at: "2026-09-01T10:00:00Z".into(),
            last_played_at: "2026-09-01T10:00:00Z".into(),
            updated_at: "2026-09-01T10:00:00Z".into(),
            ended_at: None,
            play_duration_ms: None,
            metadata: HistoryMetadata::default(),
        });
        let deleted_fav = FavouriteRecord {
            deleted_at: Some("2026-09-30T00:00:00Z".into()),
            ..fav(fav_id, "a", "2026-09-30T00:00:00Z")
        };
        let deleted_hist = HistoryRecord {
            deleted_at: Some("2026-09-30T00:00:00Z".into()),
            ..hist(hist_id, "a", "2026-09-30T00:00:00Z")
        };
        let applied = store
            .apply_incoming_records(&[deleted_fav], &[deleted_hist])
            .unwrap();
        assert_eq!(applied, 2);
        assert!(store.favourites().is_empty());
        assert!(store.history().is_empty());
        let _ = fs::remove_file(path);
    }
    #[test]
    fn synced_favourite_takes_its_name_from_synced_history() {
        let b = station("b", "https://b");
        let (mut store, path) = store(vec![b]);
        let fav_id = Uuid::new_v4();
        let fav = FavouriteRecord {
            record_id: fav_id,
            station_id: Some("b".into()),
            added_at: "2026-09-01T00:00:00Z".into(),
            updated_at: "2026-09-25T00:00:00Z".into(),
            deleted_at: None,
        };
        let mut hist = hist(Uuid::new_v4(), "b", "2026-09-25T00:00:00Z");
        hist.metadata = Some(
            serde_json::json!({"lastKnownName": "Bee station"})
                .as_object()
                .unwrap()
                .clone(),
        );
        store.apply_incoming_records(&[fav], &[hist]).unwrap();
        let favourite = store
            .favourites()
            .iter()
            .find(|f| f.record_id == fav_id)
            .unwrap();
        assert_eq!(
            favourite.metadata.last_known_name,
            Some("Bee station".into())
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn history_metadata_object_maps_into_local_metadata() {
        let b = station("b", "https://b");
        let (mut store, path) = store(vec![b]);
        let id = Uuid::new_v4();
        let mut record = hist(id, "b", "2026-09-25T00:00:00Z");
        record.metadata = Some(
            serde_json::json!({"lastKnownName": "Bee station", "source": "rockmobile"})
                .as_object()
                .unwrap()
                .clone(),
        );
        store.apply_incoming_records(&[], &[record]).unwrap();
        assert_eq!(
            store.history()[0].metadata.last_known_name,
            Some("Bee station".into())
        );
        assert_eq!(
            store.history()[0].metadata.source,
            Some("rockmobile".into())
        );
        let _ = fs::remove_file(path);
    }
}
