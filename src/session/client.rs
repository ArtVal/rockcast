//! HTTP client for RockServer account management, device pairing, and token issuance.

use std::{sync::Mutex, time::Duration};
use serde::Deserialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::rockserver::RuntimeConfig;
use super::storage::{CredentialStore, OsCredentialStore};
use super::types::{
    AccountProfile, Completion, Device, DeviceList, DeviceSession, NativeCredentials, PairingPoll,
    PairingRequest, PairingCompletionRequest, SessionError,
};

/// Serializes access-token issuance within one RockCast process.
pub(super) static SESSION_MUTEX: Mutex<()> = Mutex::new(());
const ACCESS_TOKEN_REFRESH_LEAD: Duration = Duration::from_secs(120);

pub(super) fn access_token_needs_refresh(access_expires_at: &str) -> bool {
    if access_expires_at.is_empty() {
        return true;
    }
    let Ok(expires_at) = OffsetDateTime::parse(access_expires_at, &Rfc3339) else {
        return true;
    };
    expires_at - OffsetDateTime::now_utc()
        <= time::Duration::seconds(ACCESS_TOKEN_REFRESH_LEAD.as_secs() as i64)
}

pub struct AccountClient<S = OsCredentialStore> {
    config: RuntimeConfig,
    pub(super) store: S,
}

impl<S: CredentialStore> AccountClient<S> {
    pub(crate) fn new(config: RuntimeConfig, store: S) -> Self {
        Self { config, store }
    }

    fn request(
        &self,
        method: reqwest::Method,
        route: &str,
    ) -> Result<reqwest::blocking::RequestBuilder, SessionError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|_| SessionError::Unavailable)?;
        Ok(client.request(
            method,
            format!("{}{}", self.config.base_url().trim_end_matches('/'), route),
        ))
    }

    fn json<T: for<'a> Deserialize<'a>>(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<T, SessionError> {
        let response = request.send().map_err(|_| SessionError::Unavailable)?;
        if !response.status().is_success() {
            if response.status().as_u16() == 401 {
                return Err(SessionError::Unauthorized);
            }
            return Err(if response.status().is_server_error() {
                SessionError::Unavailable
            } else {
                SessionError::Rejected
            });
        }
        response.json().map_err(|_| SessionError::Unavailable)
    }

    pub(crate) fn create_pairing(
        &self,
        device_display_name: &str,
    ) -> Result<PairingRequest, SessionError> {
        let device_display_name = device_display_name.trim();
        if device_display_name.is_empty() || device_display_name.len() > 128 {
            return Err(SessionError::Rejected);
        }
        self.json(
            self.request(reqwest::Method::POST, "/api/v1/pairing-requests")?
                .json(&serde_json::json!({
                    "device_display_name": device_display_name,
                    "device_type": std::env::consts::OS,
                    "app_version": env!("CARGO_PKG_VERSION")
                })),
        )
    }

    pub(crate) fn complete_pairing_result(
        &self,
        pairing: &PairingRequest,
    ) -> Result<(AccountProfile, NativeCredentials), PairingPoll> {
        let request = self
            .request(
                reqwest::Method::POST,
                &format!(
                    "/api/v1/pairing-requests/{}/complete",
                    pairing.pairing_request_id
                ),
            )
            .map_err(|_| PairingPoll::Unavailable)?;
        let response = request
            .json(&PairingCompletionRequest {
                desktop_token: &pairing.desktop_token,
            })
            .send()
            .map_err(|_| PairingPoll::Unavailable)?;
        let status = response.status();
        if status.as_u16() == 202 {
            return Err(PairingPoll::Pending);
        }
        if status.as_u16() == 409 {
            return Err(PairingPoll::DeviceLimit);
        }
        if status.as_u16() == 410 {
            return Err(PairingPoll::Expired);
        }
        if status.as_u16() == 401 {
            return Err(PairingPoll::Rejected);
        }
        if !status.is_success() {
            return Err(if status.is_server_error() {
                PairingPoll::Unavailable
            } else {
                PairingPoll::Rejected
            });
        }
        let result: Completion = response.json().map_err(|_| PairingPoll::Unavailable)?;
        let credentials = NativeCredentials::new(
            result.device_id.clone(),
            result.device_secret,
            result.access_token,
            result.access_expires_at,
        )
        .map_err(|_| PairingPoll::Expired)?;
        Ok((
            AccountProfile {
                device_id: result.device_id,
                account_display_name: result.account_display_name,
                device_display_name: result.device_display_name,
                device_type: result.device_type,
            },
            credentials,
        ))
    }

    pub(crate) fn save_pairing_credentials(
        &self,
        credentials: &NativeCredentials,
    ) -> Result<(), PairingPoll> {
        self.store
            .save(credentials)
            .map_err(|_| PairingPoll::SecureStorageUnavailable)
    }

    /// Issues an access token while the caller holds `SESSION_MUTEX`.
    pub(super) fn issue_device_session_without_lock(&self) -> Result<(), SessionError> {
        let current = self.store.load()?.ok_or(SessionError::Rejected)?;
        let response = self
            .request(reqwest::Method::POST, "/api/v1/auth/device-session")?
            .json(&serde_json::json!({
                "device_id": current.device_id(),
                "device_secret": current.device_secret(),
            }))
            .send()
            .map_err(|_| SessionError::Unavailable)?;
        if response.status().as_u16() == 401 {
            let invalid_credential = response
                .json::<serde_json::Value>()
                .ok()
                .and_then(|body| {
                    body.get("code")
                        .and_then(|value| value.as_str())
                        .map(str::to_owned)
                })
                .as_deref()
                == Some("device_credential_invalid");
            if invalid_credential {
                log::warn!("RockCast device credential was revoked; clearing local session.dpapi");
                let _ = self.store.clear();
                return Err(SessionError::Unauthorized);
            } else {
                log::warn!(
                    "RockCast device-session request was rejected; keeping local credentials"
                );
                return Err(SessionError::Unavailable);
            }
        }
        if !response.status().is_success() {
            return Err(if response.status().is_server_error() {
                SessionError::Unavailable
            } else {
                SessionError::Rejected
            });
        }
        let session: DeviceSession = response.json().map_err(|_| SessionError::Unavailable)?;
        NativeCredentials::new(
            current.device_id,
            current.device_secret,
            session.access_token,
            session.access_expires_at,
        )
        .and_then(|new| {
            self.store.save(&new).map(|_| {
                log::info!("RockCast native access token renewed");
            })
        })
    }

    fn ensure_fresh_access_token_without_lock(&self) -> Result<NativeCredentials, SessionError> {
        let credentials = self.store.load()?.ok_or(SessionError::Rejected)?;
        if !access_token_needs_refresh(credentials.access_expires_at()) {
            return Ok(credentials);
        }
        self.issue_device_session_without_lock()?;
        self.store.load()?.ok_or(SessionError::Rejected)
    }

    pub(crate) fn has_credentials(&self) -> Result<bool, SessionError> {
        Ok(self.store.load()?.is_some())
    }

    /// Returns a still-valid native access token for an authenticated voice session.
    pub(crate) fn voice_access_token(&self) -> Result<Option<String>, SessionError> {
        let _guard = SESSION_MUTEX
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !self.has_credentials()? {
            return Ok(None);
        }
        self.ensure_fresh_access_token_without_lock()
            .map(|credentials| Some(credentials.access_token().to_owned()))
    }

    /// Returns a native-session token for device-control without creating a
    /// second credential or pairing flow. A rejected WebSocket handshake may
    /// request one forced renewal before the caller backs off.
    pub(crate) fn device_control_access_token(
        &self,
        force_renewal: bool,
    ) -> Result<Option<String>, SessionError> {
        let _guard = SESSION_MUTEX
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !self.has_credentials()? {
            return Ok(None);
        }
        let credentials = if force_renewal {
            self.issue_device_session_without_lock()?;
            self.store.load()?.ok_or(SessionError::Rejected)?
        } else {
            self.ensure_fresh_access_token_without_lock()?
        };
        Ok(Some(credentials.access_token().to_owned()))
    }

    fn authorized(
        &self,
        method: reqwest::Method,
        route: &str,
    ) -> Result<reqwest::blocking::RequestBuilder, SessionError> {
        let credentials = self.store.load()?.ok_or(SessionError::Rejected)?;
        Ok(self
            .request(method, route)?
            .bearer_auth(credentials.access_token()))
    }

    pub(crate) fn profile(&self) -> Result<AccountProfile, SessionError> {
        self.json(self.authorized(reqwest::Method::GET, "/api/v1/account/profile")?)
    }

    pub(crate) fn devices(&self) -> Result<Vec<Device>, SessionError> {
        self.json::<DeviceList>(self.authorized(reqwest::Method::GET, "/api/v1/devices")?)
            .map(|list| list.devices)
    }

    /// Loads profile and devices, obtaining a new access token once when the current one is stale.
    pub(crate) fn load_account_session(
        &self,
    ) -> Result<Option<(AccountProfile, Vec<Device>)>, SessionError> {
        let _guard = SESSION_MUTEX
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !self.has_credentials()? {
            return Ok(None);
        }
        let _credentials = self.ensure_fresh_access_token_without_lock()?;
        match self
            .profile()
            .and_then(|profile| self.devices().map(|devices| Some((profile, devices))))
        {
            Ok(session) => Ok(session),
            Err(SessionError::Unauthorized) => {
                self.issue_device_session_without_lock().and_then(|_| {
                    let profile = self.profile()?;
                    let devices = self.devices()?;
                    Ok(Some((profile, devices)))
                })
            }
            Err(error) => Err(error),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn revoke_device(&self, device_id: &str) -> Result<(), SessionError> {
        let response = self
            .authorized(
                reqwest::Method::DELETE,
                &format!("/api/v1/devices/{device_id}"),
            )?
            .send()
            .map_err(|_| SessionError::Unavailable)?;
        if response.status().is_success() {
            Ok(())
        } else if response.status().as_u16() == 401 {
            let _guard = SESSION_MUTEX
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            self.issue_device_session_without_lock()?;
            let retry = self
                .authorized(
                    reqwest::Method::DELETE,
                    &format!("/api/v1/devices/{device_id}"),
                )?
                .send()
                .map_err(|_| SessionError::Unavailable)?;
            if retry.status().is_success() {
                Ok(())
            } else {
                Err(SessionError::Rejected)
            }
        } else {
            Err(SessionError::Rejected)
        }
    }

    pub(crate) fn logout(&self) -> Result<(), SessionError> {
        let device_id = self
            .store
            .load()?
            .ok_or(SessionError::Rejected)?
            .device_id()
            .to_owned();
        self.revoke_device(&device_id)?;
        self.store.clear()
    }
}
