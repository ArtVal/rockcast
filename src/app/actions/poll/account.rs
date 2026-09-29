//! Account loading, pairing, and session lifecycle message handling.

use crate::{
    app::{
        AccountContext, AccountErrorKind, AccountUiState, RockCastApp, account_session_active,
        default_pairing_device_name,
    },
    session::{AccountProfile, Device, PairingPoll, PairingRequest, SessionError},
};

impl RockCastApp {
    pub(super) fn handle_pairing_started(
        &mut self,
        name: String,
        result: Result<PairingRequest, SessionError>,
    ) {
        match result {
            Ok(request) => {
                self.pairing_link_copied = false;
                self.account_state = AccountUiState::Waiting {
                    request,
                    status: self.lang.t().account_waiting_title.into(),
                };
            }
            Err(_) => {
                self.account_state = AccountUiState::Disconnected {
                    device_name: name,
                    message: Some(self.lang.t().account_connection_failed.into()),
                };
            }
        }
    }

    pub(super) fn handle_account_loaded(
        &mut self,
        result: Result<Option<(AccountProfile, Vec<Device>)>, SessionError>,
    ) {
        self.account_refreshing = false;
        self.account_load_started = false;
        if matches!(self.account_state, AccountUiState::Waiting { .. }) {
            return;
        }
        let preserve_first_time =
            matches!(self.account_state, AccountUiState::ConnectedFirstTime { .. });
        let loading_account = matches!(
            self.account_state,
            AccountUiState::Starting {
                loading_account: true,
                ..
            }
        );
        let cached = match &self.account_state {
            AccountUiState::Connected { context, .. }
            | AccountUiState::ConnectedFirstTime { context } => Some(context.clone()),
            AccountUiState::Error { cached, .. } => cached.clone(),
            _ => None,
        };
        self.account_state = match result {
            Ok(Some((profile, devices))) => {
                log::info!(
                    "account session loaded for device {}",
                    profile.device_display_name
                );
                let context = AccountContext { profile, devices };
                if preserve_first_time {
                    AccountUiState::ConnectedFirstTime { context }
                } else {
                    AccountUiState::Connected {
                        context,
                        banner: None,
                    }
                }
            }
            Ok(None) => {
                log::info!("account session probe: offline");
                if preserve_first_time && !loading_account {
                    return;
                }
                AccountUiState::Disconnected {
                    device_name: default_pairing_device_name(),
                    message: None,
                }
            }
            Err(SessionError::Unauthorized) => {
                log::warn!("account session probe: credentials rejected");
                if preserve_first_time && !loading_account {
                    return;
                }
                AccountUiState::Disconnected {
                    device_name: default_pairing_device_name(),
                    message: Some(self.lang.t().account_session_reconnect.into()),
                }
            }
            Err(SessionError::SecureStorageUnavailable) => AccountUiState::Error {
                kind: AccountErrorKind::SecureStorage,
                cached: None,
            },
            Err(_) => match cached {
                Some(context) => AccountUiState::Connected {
                    context,
                    banner: Some(self.lang.t().account_unavailable.into()),
                },
                None => AccountUiState::Error {
                    kind: AccountErrorKind::Recoverable,
                    cached: None,
                },
            },
        };
        if account_session_active(&self.account_state) {
            self.schedule_personal_sync();
        }
    }

    pub(super) fn handle_pairing_result(
        &mut self,
        request_id: String,
        result: Result<AccountProfile, PairingPoll>,
    ) {
        let AccountUiState::Waiting {
            request: pairing, ..
        } = &self.account_state
        else {
            return;
        };
        if pairing.pairing_request_id != request_id {
            return;
        }
        self.pairing_cancel = None;
        match result {
            Ok(profile) => {
                self.account_load_started = false;
                self.account_refreshing = false;
                self.account_state = AccountUiState::ConnectedFirstTime {
                    context: AccountContext {
                        profile,
                        devices: Vec::new(),
                    },
                };
                // A freshly paired device starts with a clean
                // per-device sync cursor (full snapshot pull).
                self.reset_personal_sync_state();
                self.force_account_reload();
            }
            Err(PairingPoll::SecureStorageUnavailable) => {
                self.account_state = AccountUiState::Error {
                    kind: AccountErrorKind::SecureStorage,
                    cached: None,
                };
            }
            Err(PairingPoll::Unavailable) => {
                self.account_state = AccountUiState::Error {
                    kind: AccountErrorKind::Recoverable,
                    cached: None,
                };
            }
            Err(_) => {
                self.account_state = AccountUiState::Disconnected {
                    device_name: default_pairing_device_name(),
                    message: Some(self.lang.t().account_terminal_error.into()),
                };
            }
        }
    }
}
