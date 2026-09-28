//! RM-012-B personal-data sync orchestration: triggers (startup, debounced
//! local edits, periodic pull), the background cycle, and result application
//! on the UI thread. Sync never blocks the UI and never logs token material.

use std::time::{Duration, Instant};

use crate::personal_data::LocalProfile;
use crate::personal_sync::{
    HttpSyncChannel, SyncFailure, SyncOutcome, SyncState, retry_backoff, state_path, sync_once,
};
use crate::session::{AccountClient, OsCredentialStore, SessionError};

use super::super::{RockCastApp, UiMsg, account_session_active};

/// Local edits coalesce before the first push after a change.
const EDIT_DEBOUNCE: Duration = Duration::from_secs(10);
/// Periodic pull cadence; a cycle is one push+pull round trip.
const PERIODIC_PULL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SyncPhase {
    #[default]
    Idle,
    Ok,
    Error,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SyncStatus {
    pub phase: SyncPhase,
    /// Wall-clock time of the last successful sync, for the account panel.
    pub last_sync_at: Option<String>,
}

impl SyncStatus {
    pub fn metric(&self, session_active: bool) -> crate::telemetry::SyncMetric {
        match (session_active, self.phase) {
            (false, _) => crate::telemetry::SyncMetric::Off,
            (true, SyncPhase::Idle) => crate::telemetry::SyncMetric::Idle,
            (true, SyncPhase::Ok) => crate::telemetry::SyncMetric::Ok,
            (true, SyncPhase::Error) => crate::telemetry::SyncMetric::Error,
        }
    }
}

fn run_cycle(
    config: crate::rockserver::RuntimeConfig,
    profile: &LocalProfile,
    state: &SyncState,
) -> Result<SyncOutcome, SyncFailure> {
    let account = AccountClient::new(config.clone(), OsCredentialStore);
    let token = account
        .device_control_access_token(false)
        .map_err(session_failure)?
        .ok_or(SyncFailure::Unauthorized)?;
    let mut channel = HttpSyncChannel::new(config, token);
    sync_once(&mut channel, profile, state)
}

fn session_failure(error: SessionError) -> SyncFailure {
    match error {
        SessionError::Unauthorized | SessionError::Rejected => SyncFailure::Unauthorized,
        _ => SyncFailure::Unavailable,
    }
}

fn local_clock_time() -> String {
    let now = match time::OffsetDateTime::now_local() {
        Ok(local) => local,
        Err(_) => time::OffsetDateTime::now_utc(),
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute()
    )
}

impl RockCastApp {
    pub(in crate::app) fn personal_sync_ready(&self) -> bool {
        self.personal_data.is_some()
            && self.sync_state.is_some()
            && account_session_active(&self.account_state)
    }

    /// Schedules a debounced push after a local favourites/history edit.
    pub(in crate::app) fn schedule_personal_sync(&mut self) {
        if self.sync_running || !self.personal_sync_ready() {
            return;
        }
        self.sync_debounce_at = Some(Instant::now() + EDIT_DEBOUNCE);
    }

    /// Drives startup, debounced-edit, and periodic cycles from the UI loop.
    pub(in crate::app) fn tick_personal_sync(&mut self) {
        if self.sync_running
            || self.shutting_down
            || !self.personal_sync_ready()
            || self
                .sync_retry_not_before
                .is_some_and(|due| Instant::now() < due)
        {
            return;
        }
        if self
            .sync_debounce_at
            .is_some_and(|due| Instant::now() >= due)
        {
            self.sync_debounce_at = None;
            self.spawn_personal_sync();
            return;
        }
        if Instant::now() >= self.sync_last_periodic + PERIODIC_PULL {
            self.spawn_personal_sync();
        }
    }

    fn spawn_personal_sync(&mut self) {
        let (Some(profile), Some(state)) = (
            self.personal_data
                .as_ref()
                .map(|store| store.profile().clone()),
            self.sync_state.clone(),
        ) else {
            return;
        };
        let generation = self.sync_generation.wrapping_add(1);
        self.sync_generation = generation;
        self.sync_running = true;
        self.sync_last_periodic = Instant::now();
        let ui_tx = self.ui_tx.clone();
        let config = self.rockserver.clone();
        if self
            .background
            .spawn(move |_cancel| {
                let result = run_cycle(config, &profile, &state);
                let _ = ui_tx.send(UiMsg::PersonalSyncResult { generation, result });
            })
            .is_err()
        {
            self.sync_running = false;
        }
    }

    pub(in crate::app) fn handle_personal_sync_result(
        &mut self,
        generation: u64,
        result: Result<SyncOutcome, SyncFailure>,
    ) {
        if generation != self.sync_generation {
            return;
        }
        self.sync_running = false;
        match result {
            Ok(outcome) => {
                self.sync_backoff_attempts = 0;
                self.sync_retry_not_before = None;
                let applied = self.personal_data.as_mut().map_or(Ok(0), |store| {
                    store.apply_incoming_records(
                        &outcome.favourite_records,
                        &outcome.history_records,
                    )
                });
                match applied {
                    Ok(applied) => {
                        // The cursor is persisted only after the records are
                        // durably applied; a failed save merely forces a
                        // redundant re-pull next cycle.
                        if let Some(store) = self.personal_data.as_ref() {
                            let mut next = SyncState::matched_to(store.profile());
                            next.server_revision = outcome.server_revision;
                            if let Err(error) = next.save(&state_path()) {
                                log::warn!("personal sync state save failed: {error}");
                            }
                            self.sync_state = Some(next);
                        }
                        self.sync_status = SyncStatus {
                            phase: SyncPhase::Ok,
                            last_sync_at: Some(local_clock_time()),
                        };
                        log::info!(
                            "personal sync ok: pushed_favourites={} pushed_history={} applied={} batches={} cursor={}",
                            outcome.pushed_favourites,
                            outcome.pushed_history,
                            applied,
                            outcome.batches,
                            outcome.server_revision
                        );
                    }
                    Err(error) => {
                        self.record_sync_failure(&SyncFailure::LocalApply(error.to_string()));
                    }
                }
            }
            Err(failure) => self.record_sync_failure(&failure),
        }
    }

    fn record_sync_failure(&mut self, failure: &SyncFailure) {
        self.sync_backoff_attempts += 1;
        let delay = retry_backoff(self.sync_backoff_attempts - 1);
        self.sync_retry_not_before = Some(Instant::now() + delay);
        self.sync_status = SyncStatus {
            phase: SyncPhase::Error,
            last_sync_at: self.sync_status.last_sync_at.clone(),
        };
        log::warn!("personal sync failed (retry in {delay:?}): {failure}");
    }

    /// Drops the per-device cursor and base so the next cycle is a full
    /// snapshot pull. Used when the local profile is recreated (lost file) or
    /// a brand-new account session is paired, so stale "deleted everything"
    /// state can never tombstone the account.
    pub(in crate::app) fn reset_personal_sync_state(&mut self) {
        let fresh = SyncState::fresh();
        if let Err(error) = fresh.save(&state_path()) {
            log::warn!("personal sync state reset failed: {error}");
        }
        self.sync_state = Some(fresh);
        self.sync_backoff_attempts = 0;
        self.sync_retry_not_before = None;
        self.sync_debounce_at = None;
        self.sync_status = SyncStatus::default();
        self.schedule_personal_sync();
    }

    /// Loads (or re-creates) the per-device sync state after the personal
    /// profile store opens.
    pub(in crate::app) fn init_personal_sync_state(&mut self) {
        let created_fresh = self
            .personal_data
            .as_ref()
            .is_some_and(|store| store.created_fresh());
        if created_fresh {
            self.reset_personal_sync_state();
        } else if self.sync_state.is_none() {
            self.sync_state = Some(SyncState::load(&state_path()));
        }
    }
}
