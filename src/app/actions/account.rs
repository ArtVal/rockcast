//! Account background jobs, pairing initiation, and session actions.

use crate::{
    app::{
        AccountErrorKind, AccountUiState, RockCastApp, account_session_active,
        default_pairing_device_name, messages::UiMsg,
    },
    session::{AccountClient, OsCredentialStore},
};

impl RockCastApp {
    pub(in crate::app) fn ensure_account_loaded(&mut self) {
        if self.account_load_started {
            return;
        }
        self.account_load_started = true;
        self.spawn_account_load();
    }

    pub(in crate::app) fn begin_account_load(&mut self) {
        if matches!(self.account_state, AccountUiState::Waiting { .. }) {
            return;
        }
        if account_session_active(&self.account_state) {
            return;
        }
        if !self.account_load_started {
            return;
        }
        if !matches!(self.account_state, AccountUiState::Starting { .. }) {
            self.account_state = AccountUiState::Starting {
                device_name: default_pairing_device_name(),
                loading_account: true,
            };
        }
    }

    pub(in crate::app) fn force_account_reload(&mut self) {
        self.account_refreshing = false;
        self.account_load_started = true;
        self.spawn_account_load();
    }

    pub(in crate::app) fn refresh_account(&mut self) {
        if self.account_load_started || self.account_refreshing {
            return;
        }
        self.account_refreshing = true;
        self.account_load_started = true;
        if let AccountUiState::Connected { banner, .. } = &mut self.account_state {
            *banner = Some(self.lang.t().account_checking.into());
        }
        self.spawn_account_load();
    }

    fn spawn_account_load(&mut self) {
        let tx = self.ui_tx.clone();
        let config = self.rockserver.clone();
        if self
            .background
            .spawn(move |_| {
                let client = AccountClient::new(config, OsCredentialStore);
                let result = client.load_account_session();
                let _ = tx.send(UiMsg::AccountLoaded(result));
            })
            .is_err()
        {
            self.account_load_started = false;
            self.account_refreshing = false;
            self.account_state = AccountUiState::Error {
                kind: AccountErrorKind::Recoverable,
                cached: None,
            };
        }
    }

    pub(in crate::app) fn start_pairing(&mut self, name: String) {
        let fallback_name = name.clone();
        self.pairing_link_copied = false;
        self.account_state = AccountUiState::Starting {
            device_name: name.clone(),
            loading_account: false,
        };
        let tx = self.ui_tx.clone();
        let config = self.rockserver.clone();
        if self
            .background
            .spawn(move |_| {
                let result = AccountClient::new(config, OsCredentialStore).create_pairing(&name);
                let _ = tx.send(UiMsg::PairingStarted { name, result });
            })
            .is_err()
        {
            self.account_state = AccountUiState::Disconnected {
                device_name: fallback_name,
                message: Some(self.lang.t().account_connection_failed.into()),
            };
        }
    }

    pub(in crate::app) fn cancel_pairing(&mut self) {
        if let Some(cancel) = &self.pairing_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.pairing_cancel = None;
        self.pairing_link_copied = false;
        self.account_state = AccountUiState::Disconnected {
            device_name: default_pairing_device_name(),
            message: Some(self.lang.t().account_connection_cancelled.into()),
        };
    }

    pub(in crate::app) fn logout_account(&mut self) {
        let _ = AccountClient::new(self.rockserver.clone(), OsCredentialStore).logout();
        self.account_state = AccountUiState::Disconnected {
            device_name: default_pairing_device_name(),
            message: None,
        };
    }

    pub(in crate::app) fn revoke_account_device(&mut self, device_id: &str) {
        self.revoke_confirmation = None;
        if AccountClient::new(self.rockserver.clone(), OsCredentialStore)
            .revoke_device(device_id)
            .is_ok()
        {
            self.refresh_account();
        }
    }
}
