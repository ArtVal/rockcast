//! Client side of the personal-data sync (RM-012-B): favourites and playback
//! history converge across RockCast and RockMobile through the RM-012-A
//! `POST /api/v1/sync` contract. The local RM-007-A profile remains the full
//! offline fallback; every sync step is best-effort and never blocks the UI.

mod contract;
mod engine;
mod state;
mod transport;

#[cfg(test)]
mod tests;

pub use contract::{FavouriteRecord, HistoryRecord};
pub(crate) use engine::{SyncFailure, SyncOutcome, retry_backoff, sync_once};
pub(crate) use state::{SyncState, state_path};
pub(crate) use transport::HttpSyncChannel;
