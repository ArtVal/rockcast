//! DTOs, credentials, and error types for account sessions and pairing.

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SessionError {
    #[error("The account service is unavailable.")]
    Unavailable,
    #[error("The account request was rejected or has expired.")]
    Rejected,
    #[error("The account session is no longer authorized.")]
    Unauthorized,
    #[error("Secure credential storage is unavailable; RockCast remains offline.")]
    SecureStorageUnavailable,
}

#[derive(Clone, PartialEq, Eq)]
pub struct NativeCredentials {
    pub(super) device_id: String,
    pub(super) device_secret: String,
    pub(super) access_token: String,
    pub(super) access_expires_at: String,
}

impl NativeCredentials {
    pub(super) fn new(
        device_id: String,
        device_secret: String,
        access_token: String,
        access_expires_at: String,
    ) -> Result<Self, SessionError> {
        if device_id.is_empty() || device_secret.len() < 32 || access_token.len() < 16 {
            return Err(SessionError::Rejected);
        }
        Ok(Self {
            device_id,
            device_secret,
            access_token,
            access_expires_at,
        })
    }

    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub(super) fn access_expires_at(&self) -> &str {
        &self.access_expires_at
    }

    pub(super) fn device_id(&self) -> &str {
        &self.device_id
    }

    pub(super) fn device_secret(&self) -> &str {
        &self.device_secret
    }
}

#[derive(Clone, Deserialize)]
#[allow(dead_code)]
pub struct PairingRequest {
    pub(crate) pairing_request_id: String,
    pub(super) desktop_token: String,
    pub(super) approval_secret: String,
    pub(crate) short_code: String,
    pub(crate) verification_phrase: String,
    pub(crate) device_display_name: String,
    pub(crate) device_type: String,
    pub(crate) expires_at: String,
    pub(crate) status: String,
}

impl PairingRequest {
    /// Builds the first-party browser handoff with the approval secret in the URL fragment.
    pub(crate) fn deep_link(&self, base_url: &str) -> String {
        format!(
            "{}/?code={}#secret={}",
            base_url.trim_end_matches('/'),
            self.short_code,
            self.approval_secret
        )
    }
}

#[derive(Clone, Deserialize)]
#[allow(dead_code)]
pub struct AccountProfile {
    pub(crate) device_id: String,
    pub(crate) account_display_name: String,
    pub(crate) device_display_name: String,
    pub(crate) device_type: String,
}

#[derive(Clone, Deserialize)]
#[allow(dead_code)]
pub struct Device {
    pub(crate) device_id: String,
    pub(crate) device_display_name: String,
    pub(crate) device_type: String,
    #[serde(default)]
    pub(crate) created_at: String,
    #[serde(default)]
    pub(crate) last_seen_at: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct DeviceList {
    pub(super) devices: Vec<Device>,
}

#[derive(Deserialize, Serialize)]
pub(super) struct StoredCredentials {
    pub(super) device_id: String,
    pub(super) device_secret: String,
    pub(super) access_token: String,
    #[serde(default)]
    pub(super) access_expires_at: String,
}

#[derive(Deserialize)]
pub(super) struct LegacyStoredCredentials {
    #[allow(dead_code)]
    pub(super) access_token: String,
    #[allow(dead_code)]
    pub(super) refresh_token: String,
}

#[derive(Deserialize)]
pub(super) struct DeviceSession {
    pub(super) access_token: String,
    pub(super) access_expires_at: String,
}

#[derive(Deserialize)]
pub(super) struct Completion {
    pub(super) device_id: String,
    pub(super) access_token: String,
    pub(super) access_expires_at: String,
    pub(super) device_secret: String,
    pub(super) account_display_name: String,
    pub(super) device_display_name: String,
    pub(super) device_type: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PairingCompletionRequest<'a> {
    pub(super) desktop_token: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingPoll {
    Pending,
    Expired,
    DeviceLimit,
    Rejected,
    TimedOut,
    Unavailable,
    SecureStorageUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairingPollControl {
    Continue,
    Cancelled,
    TimedOut,
}

pub(crate) fn pairing_poll_control(
    cancelled: bool,
    now: std::time::Instant,
    deadline: std::time::Instant,
) -> PairingPollControl {
    if cancelled {
        PairingPollControl::Cancelled
    } else if now >= deadline {
        PairingPollControl::TimedOut
    } else {
        PairingPollControl::Continue
    }
}
