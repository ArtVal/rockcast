//! Sync transport: one `POST /api/v1/sync` round trip over the native device
//! session, with a single 401-driven session renewal per request.

use std::time::Duration;

use crate::{
    rockserver::RuntimeConfig,
    session::{AccountClient, OsCredentialStore, SessionError},
};

use super::contract::{SyncRequest, SyncResponse};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SyncTransportError {
    #[error("authentication required")]
    Unauthorized,
    #[error("sync rate limited")]
    RateLimited,
    /// `details.field` or the error code of a 422; never record payloads.
    #[error("batch rejected: {0}")]
    Rejected(String),
    #[error("sync service unavailable")]
    Unavailable,
    #[error("sync response invalid: {0}")]
    Invalid(String),
}

impl SyncTransportError {
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Unauthorized)
    }
}

/// Connection-level seam: engine tests drive a scripted fake, production uses
/// the HTTP device-session channel.
pub trait SyncChannel {
    fn send(&mut self, request: &SyncRequest) -> Result<SyncResponse, SyncTransportError>;
    /// Refreshes the native device session after an unexpected 401.
    fn refresh_session(&mut self) -> Result<(), SyncTransportError>;
}

pub struct HttpSyncChannel {
    config: RuntimeConfig,
    client: reqwest::blocking::Client,
    access_token: String,
}

impl HttpSyncChannel {
    pub fn new(config: RuntimeConfig, access_token: String) -> Self {
        Self {
            config,
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .expect("sync HTTP client"),
            access_token,
        }
    }
    fn post(&self, request: &SyncRequest) -> Result<SyncResponse, SyncTransportError> {
        let url = format!(
            "{}/api/v1/sync",
            self.config.base_url().trim_end_matches('/')
        );
        let response = self
            .client
            .post(url)
            .bearer_auth(&self.access_token)
            .json(request)
            .send()
            .map_err(|_| SyncTransportError::Unavailable)?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<SyncResponse>()
                .map_err(|error| SyncTransportError::Invalid(error.to_string()));
        }
        Err(match status.as_u16() {
            401 => SyncTransportError::Unauthorized,
            429 => SyncTransportError::RateLimited,
            422 => {
                let field = response
                    .json::<serde_json::Value>()
                    .ok()
                    .and_then(|body| {
                        body.get("details")
                            .and_then(|details| details.get("field"))
                            .and_then(|value| value.as_str())
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "unknown".into());
                SyncTransportError::Rejected(field)
            }
            400 | 413 => SyncTransportError::Rejected("request_shape".into()),
            _ if status.is_server_error() => SyncTransportError::Unavailable,
            _ => SyncTransportError::Unavailable,
        })
    }
}

impl SyncChannel for HttpSyncChannel {
    fn send(&mut self, request: &SyncRequest) -> Result<SyncResponse, SyncTransportError> {
        self.post(request)
    }
    fn refresh_session(&mut self) -> Result<(), SyncTransportError> {
        let account = AccountClient::new(self.config.clone(), OsCredentialStore);
        account
            .device_control_access_token(true)
            .map_err(|error| match error {
                SessionError::Unauthorized => SyncTransportError::Unauthorized,
                _ => SyncTransportError::Unavailable,
            })?
            .map(|token| self.access_token = token)
            .ok_or(SyncTransportError::Unauthorized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rockserver::RuntimeConfig;

    #[test]
    fn unauthorized_and_rate_limit_errors_are_recognizable() {
        assert!(SyncTransportError::Unauthorized.is_unauthorized());
        assert!(!SyncTransportError::RateLimited.is_unauthorized());
        assert!(!SyncTransportError::Rejected("favourites".into()).is_unauthorized());
        assert_eq!(
            SyncTransportError::Rejected("history".into()).to_string(),
            "batch rejected: history"
        );
    }

    #[test]
    fn http_channel_requires_an_absolute_base_url() {
        let mut channel = HttpSyncChannel::new(
            RuntimeConfig::for_test("not-a-url".into(), None),
            "token-token-token".into(),
        );
        assert_eq!(
            channel.send(&SyncRequest::default()).unwrap_err(),
            SyncTransportError::Unavailable
        );
    }
}
