//! Engine integration tests on a scripted fake channel: LWW application
//! (tombstones and echoes), cursors, batching, 401 renewal, 429/422/503
//! handling, first sync, and idempotence after a mid-apply crash.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::personal_data::{
    CatalogResolver, Favourite, HistoryMetadata, LocalProfile, PersonalDataStore,
    PlaybackHistoryEntry,
};
use crate::stations::Station;

use super::contract::{
    CollectionRecords, FavouriteRecord, HistoryRecord, SyncRequest, SyncResponse,
};
use super::engine::{SyncFailure, sync_once};
use super::state::{BATCH_LIMIT, SyncState};
use super::transport::{SyncChannel, SyncTransportError};

// ---------------------------------------------------------------------------
// fakes and helpers

struct FakeChannel {
    requests: Vec<SyncRequest>,
    script: Vec<Result<SyncResponse, SyncTransportError>>,
    renewals: usize,
}

impl FakeChannel {
    fn new(script: Vec<Result<SyncResponse, SyncTransportError>>) -> Self {
        Self {
            requests: Vec::new(),
            script,
            renewals: 0,
        }
    }
}

impl SyncChannel for FakeChannel {
    fn send(&mut self, request: &SyncRequest) -> Result<SyncResponse, SyncTransportError> {
        self.requests.push(request.clone());
        self.script.remove(0)
    }
    fn refresh_session(&mut self) -> Result<(), SyncTransportError> {
        self.renewals += 1;
        Ok(())
    }
}

fn station(id: &str) -> Station {
    Station::from_primary(
        id.into(),
        id.into(),
        format!("https://{id}"),
        "".into(),
        "".into(),
        128,
        "mp3".into(),
    )
}

fn favourite(record_id: Uuid, station: &str, updated_at: &str) -> Favourite {
    Favourite {
        record_id,
        station_id: station.into(),
        added_at: "2026-09-01T00:00:00Z".into(),
        updated_at: updated_at.into(),
        metadata: Default::default(),
    }
}

fn history(record_id: Uuid, station: &str, updated_at: &str) -> PlaybackHistoryEntry {
    PlaybackHistoryEntry {
        record_id,
        station_id: station.into(),
        started_at: "2026-09-05T10:00:00Z".into(),
        last_played_at: "2026-09-05T10:05:00Z".into(),
        updated_at: updated_at.into(),
        ended_at: None,
        play_duration_ms: Some(1_000),
        metadata: HistoryMetadata::default(),
    }
}

fn profile_with(favourites: Vec<Favourite>, history: Vec<PlaybackHistoryEntry>) -> LocalProfile {
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

fn ok_response(
    revision: u64,
    favourites: Vec<FavouriteRecord>,
    history: Vec<HistoryRecord>,
) -> Result<SyncResponse, SyncTransportError> {
    Ok(SyncResponse {
        server_revision: revision,
        server_time: "2026-09-28T12:00:00Z".into(),
        favourites: CollectionRecords {
            records: favourites,
        },
        history: CollectionRecords { records: history },
    })
}

fn fav_record(
    record_id: Uuid,
    station: &str,
    updated_at: &str,
    deleted: Option<&str>,
) -> FavouriteRecord {
    FavouriteRecord {
        record_id,
        station_id: Some(station.into()),
        added_at: "2026-09-01T00:00:00Z".into(),
        updated_at: updated_at.into(),
        deleted_at: deleted.map(str::to_owned),
    }
}

/// A store whose profile file is exactly `profile`, resolved against the
/// stations it references.
fn store_with(profile: &LocalProfile) -> (PersonalDataStore, PathBuf) {
    let path = std::env::temp_dir().join(format!("rockcast-sync-{}.json", Uuid::new_v4()));
    std::fs::write(&path, serde_json::to_vec(profile).unwrap()).unwrap();
    let station_ids: Vec<&str> = profile
        .favourites
        .iter()
        .map(|f| f.station_id.as_str())
        .chain(
            profile
                .playback_history
                .iter()
                .map(|e| e.station_id.as_str()),
        )
        .collect();
    let stations: Vec<Station> = station_ids.iter().map(|id| station(id)).collect();
    (
        PersonalDataStore::open(
            path.clone(),
            CatalogResolver::from_stations(&stations, None),
        )
        .unwrap(),
        path,
    )
}

fn empty_store() -> (PersonalDataStore, PathBuf) {
    let path = std::env::temp_dir().join(format!("rockcast-sync-{}.json", Uuid::new_v4()));
    (
        PersonalDataStore::open(path.clone(), CatalogResolver::from_stations(&[], None)).unwrap(),
        path,
    )
}

/// Mirrors the UI-thread post-processing after a successful cycle.
fn apply_and_advance(
    store: &mut PersonalDataStore,
    state_path: &Path,
    outcome: &super::engine::SyncOutcome,
) -> SyncState {
    store
        .apply_incoming_records(&outcome.favourite_records, &outcome.history_records)
        .unwrap();
    let mut next = SyncState::matched_to(store.profile());
    next.server_revision = outcome.server_revision;
    next.save(state_path).unwrap();
    next
}

// ---------------------------------------------------------------------------
// tests

#[test]
fn first_sync_pushes_everything_and_applies_the_snapshot() {
    let local_id = Uuid::new_v4();
    let remote_id = Uuid::new_v4();
    let profile = profile_with(
        vec![favourite(local_id, "a", "2026-09-20T00:00:00Z")],
        vec![],
    );
    let mut channel = FakeChannel::new(vec![ok_response(
        5,
        vec![
            fav_record(local_id, "a", "2026-09-20T00:00:00Z", None),
            fav_record(remote_id, "b", "2026-09-18T00:00:00Z", None),
        ],
        vec![],
    )]);
    let outcome = sync_once(&mut channel, &profile, &SyncState::fresh()).unwrap();
    assert_eq!(outcome.batches, 1);
    assert_eq!(outcome.server_revision, 5);
    assert_eq!(outcome.pushed_favourites, 1);
    let pushed = &channel.requests[0].favourites.as_ref().unwrap().upserts;
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].record_id, local_id);
    assert_eq!(channel.requests[0].since_revision, None);

    let (mut store, path) = empty_store();
    let state_path = path.with_extension("sync.json");
    let state = apply_and_advance(&mut store, &state_path, &outcome);
    assert_eq!(state.server_revision, 5);
    assert_eq!(store.favourites().len(), 2);
    assert!(store.is_favourite("a") && store.is_favourite("b"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn second_sync_is_pull_only_with_the_persisted_cursor() {
    let id = Uuid::new_v4();
    let profile = profile_with(vec![favourite(id, "a", "2026-09-20T00:00:00Z")], vec![]);
    let mut state = SyncState::matched_to(&profile);
    state.server_revision = 12;
    let mut channel = FakeChannel::new(vec![ok_response(13, vec![], vec![])]);
    let outcome = sync_once(&mut channel, &profile, &state).unwrap();
    assert_eq!(outcome.pushed_favourites, 0);
    assert_eq!(outcome.pushed_history, 0);
    assert_eq!(channel.requests[0].since_revision, Some(12));
    assert!(channel.requests[0].favourites.is_none());
}

#[test]
fn losing_push_echoes_and_remote_tombstones_apply_by_lww() {
    let local_id = Uuid::new_v4();
    let remote_deleted = Uuid::new_v4();
    let profile = profile_with(
        vec![
            favourite(local_id, "a", "2026-09-20T00:00:00Z"),
            favourite(remote_deleted, "b", "2026-09-01T00:00:00Z"),
        ],
        vec![],
    );
    let mut state = SyncState::matched_to(&profile);
    state.server_revision = 3;
    // Locally the favourite for "a" was edited; the server answers with an
    // older echo of it and a newer tombstone for "b".
    let edited = profile_with(
        vec![
            favourite(local_id, "a", "2026-09-25T00:00:00Z"),
            favourite(remote_deleted, "b", "2026-09-01T00:00:00Z"),
        ],
        vec![],
    );
    let mut channel = FakeChannel::new(vec![ok_response(
        9,
        vec![
            fav_record(local_id, "a", "2026-09-20T00:00:00Z", None),
            fav_record(
                remote_deleted,
                "b",
                "2026-09-27T00:00:00Z",
                Some("2026-09-27T00:00:00Z"),
            ),
        ],
        vec![],
    )]);
    let outcome = sync_once(&mut channel, &edited, &state).unwrap();
    let (mut store, path) = store_with(&edited);
    let applied = store
        .apply_incoming_records(&outcome.favourite_records, &outcome.history_records)
        .unwrap();
    // The older echo loses; the tombstone deletes.
    assert_eq!(applied, 1);
    assert!(!store.is_favourite("b"));
    assert!(store.is_favourite("a"));
    assert_eq!(store.favourites()[0].updated_at, "2026-09-25T00:00:00Z");
    let state_path = path.with_extension("sync.json");
    let next = apply_and_advance(&mut store, &state_path, &outcome);
    assert_eq!(next.server_revision, 9);
    let _ = std::fs::remove_file(path);
}

#[test]
fn local_deletion_pushes_a_tombstone_that_beats_a_future_dated_base() {
    let removed = Uuid::new_v4();
    let profile = profile_with(
        vec![favourite(removed, "a", "2999-01-01T00:00:00Z")],
        vec![],
    );
    let mut state = SyncState::matched_to(&profile);
    state.server_revision = 4;
    let emptied = profile_with(vec![], vec![]);
    let mut channel = FakeChannel::new(vec![ok_response(6, vec![], vec![])]);
    let outcome = sync_once(&mut channel, &emptied, &state).unwrap();
    let deletes = &channel.requests[0].favourites.as_ref().unwrap().deletes;
    assert_eq!(deletes.len(), 1);
    assert_eq!(deletes[0].record_id, removed);
    assert!(
        crate::personal_data::parse_time(&deletes[0].updated_at).unwrap()
            > crate::personal_data::parse_time("2999-01-01T00:00:00Z").unwrap()
    );
    assert_eq!(outcome.server_revision, 6);
}

#[test]
fn oversized_push_is_delivered_in_chunks_with_threaded_cursors() {
    let mut favourites = Vec::new();
    for _ in 0..(BATCH_LIMIT + 1) {
        favourites.push(favourite(Uuid::new_v4(), "a", "2026-09-01T00:00:00Z"));
    }
    let profile = profile_with(favourites, vec![]);
    let mut channel = FakeChannel::new(vec![
        ok_response(20, vec![], vec![]),
        ok_response(21, vec![], vec![]),
    ]);
    let outcome = sync_once(&mut channel, &profile, &SyncState::fresh()).unwrap();
    assert_eq!(channel.requests.len(), 2);
    assert_eq!(
        channel.requests[0]
            .favourites
            .as_ref()
            .unwrap()
            .upserts
            .len(),
        BATCH_LIMIT
    );
    assert_eq!(
        channel.requests[1]
            .favourites
            .as_ref()
            .unwrap()
            .upserts
            .len(),
        1
    );
    assert_eq!(outcome.server_revision, 21);
    assert_eq!(outcome.batches, 2);
}

#[test]
fn history_push_and_pull_round_trip_preserves_records() {
    let id = Uuid::new_v4();
    let profile = profile_with(vec![], vec![history(id, "a", "2026-09-20T00:00:00Z")]);
    let mut channel = FakeChannel::new(vec![ok_response(4, vec![], vec![])]);
    sync_once(&mut channel, &profile, &SyncState::fresh()).unwrap();
    let pushed = &channel.requests[0].history.as_ref().unwrap().upserts;
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].record_id, id);
    assert_eq!(pushed[0].play_duration_ms, Some(1_000));
}

#[test]
fn unexpected_401_renews_the_session_and_retries_once() {
    let profile = profile_with(vec![], vec![]);
    let mut channel = FakeChannel::new(vec![
        Err(SyncTransportError::Unauthorized),
        ok_response(2, vec![], vec![]),
    ]);
    let outcome = sync_once(&mut channel, &profile, &SyncState::fresh()).unwrap();
    assert_eq!(outcome.server_revision, 2);
    assert_eq!(channel.renewals, 1);
    assert_eq!(channel.requests.len(), 2);
}

#[test]
fn renewed_session_that_is_still_unauthorized_fails_without_a_loop() {
    let profile = profile_with(vec![], vec![]);
    let mut channel = FakeChannel::new(vec![
        Err(SyncTransportError::Unauthorized),
        Err(SyncTransportError::Unauthorized),
    ]);
    assert!(matches!(
        sync_once(&mut channel, &profile, &SyncState::fresh()),
        Err(SyncFailure::Unauthorized)
    ));
    assert_eq!(channel.renewals, 1);
}

#[test]
fn rate_limit_and_outage_failures_map_to_backoff_classes() {
    let profile = profile_with(vec![], vec![]);
    for error in [
        SyncTransportError::RateLimited,
        SyncTransportError::Unavailable,
    ] {
        let mut channel = FakeChannel::new(vec![Err(error.clone())]);
        let failure = sync_once(&mut channel, &profile, &SyncState::fresh()).unwrap_err();
        assert!(matches!(
            failure,
            SyncFailure::RateLimited | SyncFailure::Unavailable
        ));
    }
}

#[test]
fn validation_rejection_surfaces_the_field_only() {
    let profile = profile_with(vec![], vec![]);
    let mut channel = FakeChannel::new(vec![Err(SyncTransportError::Rejected(
        "favourites.upserts[3].station_id".into(),
    ))]);
    let failure = sync_once(&mut channel, &profile, &SyncState::fresh()).unwrap_err();
    assert_eq!(
        failure.to_string(),
        "sync batch rejected: favourites.upserts[3].station_id"
    );
    // The next cycle (after the data is fixed) starts from scratch.
    let mut recovered = FakeChannel::new(vec![ok_response(3, vec![], vec![])]);
    assert_eq!(
        sync_once(&mut recovered, &profile, &SyncState::fresh())
            .unwrap()
            .server_revision,
        3
    );
}

#[test]
fn crash_mid_apply_is_idempotent_on_the_next_cycle() {
    // Cycle 1 succeeded on the server (cursor would be 8) but the client
    // crashed before applying anything or persisting the cursor.
    let local_id = Uuid::new_v4();
    let remote_id = Uuid::new_v4();
    let profile = profile_with(
        vec![favourite(local_id, "a", "2026-09-20T00:00:00Z")],
        vec![],
    );
    let state = SyncState::fresh();
    let response = || {
        ok_response(
            8,
            vec![
                fav_record(local_id, "a", "2026-09-20T00:00:00Z", None),
                fav_record(remote_id, "b", "2026-09-19T00:00:00Z", None),
            ],
            vec![],
        )
    };
    let mut first = FakeChannel::new(vec![response()]);
    let first_outcome = sync_once(&mut first, &profile, &state).unwrap();

    // Retry after the crash: same stale state, server repeats the delta and
    // the push echo.
    let mut second = FakeChannel::new(vec![response()]);
    let second_outcome = sync_once(&mut second, &profile, &state).unwrap();
    assert_eq!(second_outcome.favourite_records.len(), 2);
    assert_eq!(second_outcome.server_revision, 8);

    // Applying once (or twice) converges to the same profile.
    let (mut store, path) = empty_store();
    store
        .apply_incoming_records(&first_outcome.favourite_records, &[])
        .unwrap();
    let after_first = store.favourites().to_vec();
    store
        .apply_incoming_records(&second_outcome.favourite_records, &[])
        .unwrap();
    assert_eq!(store.favourites(), after_first);
    assert_eq!(store.favourites().len(), 2);
    let _ = std::fs::remove_file(path);
}
