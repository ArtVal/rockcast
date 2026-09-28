//! One push+pull sync cycle (RM-012-B): diff against the last acknowledged
//! base, deliver batch chunks with a single 401-driven session renewal per
//! request, and hand the merged pull records plus the new cursor to the UI
//! thread, which applies them and only then persists the cursor.

use std::time::Duration;

use crate::personal_data::LocalProfile;

use super::contract::{FavouriteRecord, HistoryRecord, SyncRequest, SyncResponse};
use super::state::{self, SyncState};
use super::transport::{SyncChannel, SyncTransportError};

#[derive(Debug, thiserror::Error)]
pub enum SyncFailure {
    #[error("sync rate limited")]
    RateLimited,
    #[error("sync batch rejected: {0}")]
    Rejected(String),
    #[error("account session unauthorized")]
    Unauthorized,
    #[error("sync service unavailable")]
    Unavailable,
    #[error("local apply failed: {0}")]
    LocalApply(String),
    #[error("sync response invalid: {0}")]
    Invalid(String),
}

impl From<SyncTransportError> for SyncFailure {
    fn from(error: SyncTransportError) -> Self {
        match error {
            SyncTransportError::Unauthorized => Self::Unauthorized,
            SyncTransportError::RateLimited => Self::RateLimited,
            SyncTransportError::Rejected(field) => Self::Rejected(field),
            SyncTransportError::Unavailable => Self::Unavailable,
            SyncTransportError::Invalid(details) => Self::Invalid(details),
        }
    }
}

#[derive(Debug, Default)]
pub struct SyncOutcome {
    /// Every record the caller must apply (delta plus echoes of pushed
    /// records, including tombstones).
    pub favourite_records: Vec<FavouriteRecord>,
    pub history_records: Vec<HistoryRecord>,
    /// Cursor to persist only after the records are durably applied.
    pub server_revision: u64,
    pub batches: usize,
    pub pushed_favourites: usize,
    pub pushed_history: usize,
}

/// Exponential backoff for 429/5xx cycles: 1, 2, 4, … minutes, capped at 15.
pub(crate) fn retry_backoff(failed_attempts: u32) -> Duration {
    let shift = failed_attempts.min(4);
    Duration::from_secs(60u64.saturating_mul(1 << shift))
}

pub(crate) fn sync_once(
    channel: &mut dyn SyncChannel,
    profile: &LocalProfile,
    state: &SyncState,
) -> Result<SyncOutcome, SyncFailure> {
    let (favourites, history) = state.diff(profile);
    let pushed_favourites = favourites.upserts.len() + favourites.deletes.len();
    let pushed_history = history.upserts.len() + history.deletes.len();
    let requests = state::chunk_requests(favourites, history, state.server_revision);
    let mut outcome = SyncOutcome {
        pushed_favourites,
        pushed_history,
        ..SyncOutcome::default()
    };
    // A pull-only cycle still needs one request; chunk_requests may return an
    // empty vector when nothing is pending.
    let requests = if requests.is_empty() {
        vec![SyncRequest {
            since_revision: (state.server_revision > 0).then_some(state.server_revision),
            favourites: None,
            history: None,
        }]
    } else {
        requests
    };
    for request in requests {
        let response = deliver(channel, &request)?;
        outcome.batches += 1;
        outcome
            .favourite_records
            .extend(response.favourites.records);
        outcome.history_records.extend(response.history.records);
        outcome.server_revision = response.server_revision;
    }
    Ok(outcome)
}

/// Sends one request, refreshing the native session exactly once when the
/// server answers 401 `authentication_required`.
fn deliver(
    channel: &mut dyn SyncChannel,
    request: &SyncRequest,
) -> Result<SyncResponse, SyncFailure> {
    match channel.send(request) {
        Err(error) if error.is_unauthorized() => {
            channel.refresh_session()?;
            channel.send(request).map_err(SyncFailure::from)
        }
        result => result.map_err(SyncFailure::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_is_capped() {
        assert_eq!(retry_backoff(0), Duration::from_secs(60));
        assert_eq!(retry_backoff(1), Duration::from_secs(120));
        assert_eq!(retry_backoff(2), Duration::from_secs(240));
        assert_eq!(retry_backoff(9), Duration::from_secs(60 * 16));
    }
}
