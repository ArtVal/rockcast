//! Wire contract for `POST /api/v1/sync` (RockServer OpenAPI 0.6.0, RM-012-A).
//!
//! Field names are snake_case exactly as published; the local RM-007-A profile
//! stays camelCase and is translated in [`super::state`].

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SyncRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favourites: Option<FavouriteChanges>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryChanges>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct FavouriteChanges {
    pub upserts: Vec<FavouritePush>,
    pub deletes: Vec<RecordDelete>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HistoryChanges {
    pub upserts: Vec<HistoryPush>,
    pub deletes: Vec<RecordDelete>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FavouritePush {
    pub record_id: Uuid,
    pub station_id: String,
    pub added_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryPush {
    pub record_id: Uuid,
    pub station_id: String,
    pub started_at: String,
    pub last_played_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordDelete {
    pub record_id: Uuid,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncResponse {
    pub server_revision: u64,
    pub server_time: String,
    pub favourites: CollectionRecords<FavouriteRecord>,
    pub history: CollectionRecords<HistoryRecord>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CollectionRecords<T> {
    pub records: Vec<T>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FavouriteRecord {
    pub record_id: Uuid,
    #[serde(default)]
    pub station_id: Option<String>,
    pub added_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryRecord {
    pub record_id: Uuid,
    #[serde(default)]
    pub station_id: Option<String>,
    pub started_at: String,
    pub last_played_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub play_duration_ms: Option<u64>,
    #[serde(default)]
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
    pub updated_at: String,
    #[serde(default)]
    pub deleted_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_only_request_omits_collections() {
        let body = serde_json::to_value(SyncRequest {
            since_revision: Some(7),
            favourites: None,
            history: None,
        })
        .unwrap();
        assert_eq!(body, serde_json::json!({"since_revision": 7}));
        let first = serde_json::to_value(SyncRequest::default()).unwrap();
        assert_eq!(first, serde_json::json!({}));
    }

    #[test]
    fn favourite_push_serializes_snake_case() {
        let body = serde_json::to_value(FavouriteChanges {
            upserts: vec![FavouritePush {
                record_id: "81408a3e-5f0b-4d7a-9a1d-1f2f3d4d5e6f".parse().unwrap(),
                station_id: "station-a".into(),
                added_at: "2026-09-01T00:00:00Z".into(),
                updated_at: "2026-09-02T00:00:00Z".into(),
            }],
            deletes: vec![RecordDelete {
                record_id: "01408a3e-5f0b-4d7a-9a1d-1f2f3d4d5e6f".parse().unwrap(),
                updated_at: "2026-09-03T00:00:00Z".into(),
            }],
        })
        .unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "upserts": [{
                    "record_id": "81408a3e-5f0b-4d7a-9a1d-1f2f3d4d5e6f",
                    "station_id": "station-a",
                    "added_at": "2026-09-01T00:00:00Z",
                    "updated_at": "2026-09-02T00:00:00Z"
                }],
                "deletes": [{
                    "record_id": "01408a3e-5f0b-4d7a-9a1d-1f2f3d4d5e6f",
                    "updated_at": "2026-09-03T00:00:00Z"
                }]
            })
        );
    }

    #[test]
    fn response_parses_records_and_tombstones() {
        let body = r#"{
            "server_revision": 11,
            "server_time": "2026-09-28T12:00:00Z",
            "favourites": {"records": [
                {"record_id": "81408a3e-5f0b-4d7a-9a1d-1f2f3d4d5e6f", "station_id": "station-a",
                 "added_at": "2026-09-01T00:00:00Z", "updated_at": "2026-09-02T00:00:00Z"},
                {"record_id": "01408a3e-5f0b-4d7a-9a1d-1f2f3d4d5e6f", "station_id": "station-b",
                 "added_at": "2026-09-01T00:00:00Z", "updated_at": "2026-09-04T00:00:00Z",
                 "deleted_at": "2026-09-04T00:00:00Z"}
            ]},
            "history": {"records": [{
                "record_id": "92408a3e-5f0b-4d7a-9a1d-1f2f3d4d5e6f", "station_id": "station-a",
                "started_at": "2026-09-05T10:00:00Z", "last_played_at": "2026-09-05T10:05:00Z",
                "ended_at": "2026-09-05T10:05:00Z", "play_duration_ms": 300000,
                "metadata": {"lastKnownName": "A", "client": "rockmobile"},
                "updated_at": "2026-09-05T10:05:00Z"
            }]}
        }"#;
        let response: SyncResponse = serde_json::from_str(body).unwrap();
        assert_eq!(response.server_revision, 11);
        assert_eq!(response.favourites.records.len(), 2);
        assert!(response.favourites.records[1].deleted_at.is_some());
        let history = &response.history.records[0];
        assert_eq!(history.play_duration_ms, Some(300_000));
        assert_eq!(
            history.metadata.as_ref().unwrap().get("client"),
            Some(&serde_json::json!("rockmobile"))
        );
    }
}
