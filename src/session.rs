//! Native account session client. Tokens never implement `Debug` or persistence formats.
use crate::{rockserver::RuntimeConfig, settings};
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf, sync::Mutex, time::Duration};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// Serializes access-token issuance within one RockCast process.
static SESSION_MUTEX: Mutex<()> = Mutex::new(());
const ACCESS_TOKEN_REFRESH_LEAD: Duration = Duration::from_secs(120);

fn access_token_needs_refresh(access_expires_at: &str) -> bool {
    if access_expires_at.is_empty() {
        return true;
    }
    let Ok(expires_at) = OffsetDateTime::parse(access_expires_at, &Rfc3339) else {
        return true;
    };
    expires_at - OffsetDateTime::now_utc()
        <= time::Duration::seconds(ACCESS_TOKEN_REFRESH_LEAD.as_secs() as i64)
}

#[derive(Clone, PartialEq, Eq)]
pub struct NativeCredentials {
    device_id: String,
    device_secret: String,
    access_token: String,
    access_expires_at: String,
}

impl NativeCredentials {
    fn new(
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
    pub(crate) fn access_token(&self) -> &str {
        &self.access_token
    }
    fn access_expires_at(&self) -> &str {
        &self.access_expires_at
    }
    fn device_id(&self) -> &str {
        &self.device_id
    }
    fn device_secret(&self) -> &str {
        &self.device_secret
    }
}

#[derive(Clone, Deserialize)]
#[allow(dead_code)]
pub struct PairingRequest {
    pub(crate) pairing_request_id: String,
    desktop_token: String,
    approval_secret: String,
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
struct DeviceList {
    devices: Vec<Device>,
}
#[derive(Deserialize, Serialize)]
struct StoredCredentials {
    device_id: String,
    device_secret: String,
    access_token: String,
    #[serde(default)]
    access_expires_at: String,
}
#[derive(Deserialize)]
struct LegacyStoredCredentials {
    #[allow(dead_code)]
    access_token: String,
    #[allow(dead_code)]
    refresh_token: String,
}
#[derive(Deserialize)]
struct DeviceSession {
    access_token: String,
    access_expires_at: String,
}
#[derive(Deserialize)]
struct Completion {
    device_id: String,
    access_token: String,
    access_expires_at: String,
    device_secret: String,
    account_display_name: String,
    device_display_name: String,
    device_type: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairingCompletionRequest<'a> {
    desktop_token: &'a str,
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

/// Small seam for deterministic tests and unsupported OS handling.
pub trait CredentialStore: Send + Sync {
    fn load(&self) -> Result<Option<NativeCredentials>, SessionError>;
    fn save(&self, credentials: &NativeCredentials) -> Result<(), SessionError>;
    fn clear(&self) -> Result<(), SessionError>;
}

pub struct OsCredentialStore;
impl OsCredentialStore {
    fn path() -> Result<PathBuf, SessionError> {
        settings::app_dir()
            .map(|d| d.join("session.dpapi"))
            .ok_or(SessionError::SecureStorageUnavailable)
    }

    fn recovery_path() -> Result<PathBuf, SessionError> {
        Ok(Self::path()?.with_extension("dpapi.recovery"))
    }
}

#[cfg(windows)]
fn read_credentials(path: &PathBuf) -> Result<Option<NativeCredentials>, SessionError> {
    let encrypted = match fs::read(path) {
        Ok(value) => value,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(SessionError::SecureStorageUnavailable),
    };
    let plain = dpapi(false, &encrypted)?;
    decode_credentials(&plain)
}

/// Decodes the current credential format and treats a successfully decrypted legacy refresh blob as absent.
fn decode_credentials(plain: &[u8]) -> Result<Option<NativeCredentials>, SessionError> {
    match serde_json::from_slice::<StoredCredentials>(plain) {
        Ok(credentials) => NativeCredentials::new(
            credentials.device_id,
            credentials.device_secret,
            credentials.access_token,
            credentials.access_expires_at,
        )
        .map(Some),
        Err(_) if serde_json::from_slice::<LegacyStoredCredentials>(plain).is_ok() => Ok(None),
        Err(_) => Err(SessionError::SecureStorageUnavailable),
    }
}

#[cfg(windows)]
fn write_credentials(path: &PathBuf, credentials: &NativeCredentials) -> Result<(), SessionError> {
    let plain = serde_json::to_vec(&StoredCredentials {
        device_id: credentials.device_id.clone(),
        device_secret: credentials.device_secret.clone(),
        access_token: credentials.access_token.clone(),
        access_expires_at: credentials.access_expires_at.clone(),
    })
    .map_err(|_| SessionError::SecureStorageUnavailable)?;
    let encrypted = dpapi(true, &plain)?;
    let parent = path
        .parent()
        .ok_or(SessionError::SecureStorageUnavailable)?;
    fs::create_dir_all(parent).map_err(|_| SessionError::SecureStorageUnavailable)?;
    let temp = path.with_extension("new");
    fs::write(&temp, &encrypted).map_err(|_| SessionError::SecureStorageUnavailable)?;
    match fs::rename(&temp, path) {
        Ok(()) => {}
        Err(_) => {
            fs::write(path, &encrypted).map_err(|_| SessionError::SecureStorageUnavailable)?;
            let _ = fs::remove_file(&temp);
        }
    }
    Ok(())
}

#[cfg(windows)]
impl CredentialStore for OsCredentialStore {
    fn load(&self) -> Result<Option<NativeCredentials>, SessionError> {
        let path = Self::path()?;
        if let Some(credentials) = read_credentials(&path)? {
            return Ok(Some(credentials));
        }
        let recovery = Self::recovery_path()?;
        if let Some(credentials) = read_credentials(&recovery)? {
            log::warn!("promoting recovered RockCast native session credentials");
            let _ = self.save(&credentials);
            return Ok(Some(credentials));
        }
        Ok(None)
    }
    fn save(&self, credentials: &NativeCredentials) -> Result<(), SessionError> {
        let path = Self::path()?;
        let recovery = Self::recovery_path()?;
        write_credentials(&recovery, credentials)?;
        write_credentials(&path, credentials)?;
        let _ = fs::remove_file(&recovery);
        let persisted = read_credentials(&path)?.ok_or(SessionError::SecureStorageUnavailable)?;
        if persisted.device_secret() != credentials.device_secret()
            || persisted.device_id() != credentials.device_id()
            || persisted.access_token() != credentials.access_token()
            || persisted.access_expires_at() != credentials.access_expires_at()
        {
            return Err(SessionError::SecureStorageUnavailable);
        }
        Ok(())
    }
    fn clear(&self) -> Result<(), SessionError> {
        let path = Self::path()?;
        let recovery = Self::recovery_path()?;
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(SessionError::SecureStorageUnavailable),
        }
        match fs::remove_file(recovery) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(SessionError::SecureStorageUnavailable),
        }
    }
}

#[cfg(windows)]
fn dpapi(protect: bool, input: &[u8]) -> Result<Vec<u8>, SessionError> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    let source = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: DPAPI copies the supplied bytes; output is released with LocalFree below.
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &source,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &source,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 || output.pbData.is_null() {
        return Err(SessionError::SecureStorageUnavailable);
    }
    // SAFETY: DPAPI returned an allocated buffer of cbData bytes.
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    // SAFETY: ownership of this DPAPI allocation is transferred to LocalFree.
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(result)
}

#[cfg(not(windows))]
impl CredentialStore for OsCredentialStore {
    fn load(&self) -> Result<Option<NativeCredentials>, SessionError> {
        Err(SessionError::SecureStorageUnavailable)
    }
    fn save(&self, _: &NativeCredentials) -> Result<(), SessionError> {
        Err(SessionError::SecureStorageUnavailable)
    }
    fn clear(&self) -> Result<(), SessionError> {
        Err(SessionError::SecureStorageUnavailable)
    }
}

pub(crate) struct AccountClient<S = OsCredentialStore> {
    config: RuntimeConfig,
    store: S,
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
    fn issue_device_session_without_lock(&self) -> Result<(), SessionError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::Mutex,
        thread,
    };
    struct Memory(Mutex<Option<NativeCredentials>>);
    impl CredentialStore for Memory {
        fn load(&self) -> Result<Option<NativeCredentials>, SessionError> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, c: &NativeCredentials) -> Result<(), SessionError> {
            *self.0.lock().unwrap() = Some(c.clone());
            Ok(())
        }
        fn clear(&self) -> Result<(), SessionError> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }
    const FRESH_ACCESS_EXPIRES_AT: &str = "2030-01-01T12:00:00Z";

    fn stored_credentials(access_token: &str) -> NativeCredentials {
        NativeCredentials::new(
            "device".into(),
            "b".repeat(43),
            access_token.into(),
            FRESH_ACCESS_EXPIRES_AT.into(),
        )
        .unwrap()
    }

    #[test]
    fn access_token_needs_refresh_only_when_unknown_or_near_expiry() {
        assert!(!access_token_needs_refresh(FRESH_ACCESS_EXPIRES_AT));
        assert!(!access_token_needs_refresh("2030-01-01T12:00:00.123456Z"));
        assert!(access_token_needs_refresh(""));
        assert!(access_token_needs_refresh(
            &OffsetDateTime::now_utc().format(&Rfc3339).unwrap()
        ));
    }

    #[test]
    fn legacy_stored_credentials_without_expiry_still_load() {
        let legacy = br#"{"device_id":"device","device_secret":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","access_token":"aaaaaaaaaaaaaaaa"}"#;
        let credentials = decode_credentials(legacy).unwrap().expect("legacy session");
        assert_eq!(credentials.access_expires_at(), "");
        assert!(access_token_needs_refresh(credentials.access_expires_at()));
    }

    #[test]
    fn secrets_do_not_format_or_serialize() {
        let credentials = stored_credentials("a".repeat(16).as_str());
        let store = Memory(Mutex::new(Some(credentials)));
        assert_eq!(store.load().unwrap().unwrap().access_token().len(), 16);
    }
    #[test]
    fn unavailable_secure_store_fails_closed() {
        #[cfg(not(windows))]
        assert_eq!(
            OsCredentialStore.load().unwrap_err(),
            SessionError::SecureStorageUnavailable
        );
    }

    #[test]
    fn legacy_refresh_blob_is_an_absent_session_not_a_storage_failure() {
        let legacy = br#"{"access_token":"aaaaaaaaaaaaaaaa","refresh_token":"bbbbbbbbbbbbbbbb"}"#;
        assert!(decode_credentials(legacy).unwrap().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn legacy_dpapi_blob_is_an_absent_session_not_a_storage_failure() {
        let path = std::env::temp_dir().join(format!(
            "rockcast-legacy-session-{}.dpapi",
            uuid::Uuid::new_v4()
        ));
        let legacy = br#"{"access_token":"aaaaaaaaaaaaaaaa","refresh_token":"bbbbbbbbbbbbbbbb"}"#;
        fs::write(&path, dpapi(true, legacy).unwrap()).unwrap();
        assert!(read_credentials(&path).unwrap().is_none());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn pairing_poll_stops_only_for_cancel_or_deadline() {
        let now = std::time::Instant::now();
        assert_eq!(
            pairing_poll_control(false, now, now + Duration::from_secs(1)),
            PairingPollControl::Continue
        );
        assert_eq!(
            pairing_poll_control(true, now, now + Duration::from_secs(1)),
            PairingPollControl::Cancelled
        );
        assert_eq!(
            pairing_poll_control(false, now + Duration::from_secs(1), now),
            PairingPollControl::TimedOut
        );
    }

    fn server(status: u16, body: &'static str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while let Ok(read) = stream.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&request[..end]);
                    let length = header
                        .lines()
                        .find_map(|line| line.strip_prefix("Content-Length: "))
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let response = format!(
                "HTTP/1.1 {status} test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://{address}"), thread)
    }

    fn client(url: String, store: Memory) -> AccountClient<Memory> {
        AccountClient::new(RuntimeConfig::for_test(url, None), store)
    }

    #[test]
    fn offline_pairing_returns_safe_error_without_credentials() {
        let client = client("http://127.0.0.1:1".into(), Memory(Mutex::new(None)));
        assert!(matches!(
            client.create_pairing("RockCast — offline"),
            Err(SessionError::Unavailable)
        ));
    }

    #[test]
    fn create_pairing_uses_g1_contract_without_authorization() {
        let body = r#"{"pairing_request_id":"request","desktop_token":"aaaaaaaaaaaaaaaa","approval_secret":"bbbbbbbbbbbbbbbb","short_code":"AB12CD34","verification_phrase":"AMBER-DAWN","device_display_name":"RockCast — test","device_type":"windows","expires_at":"2026-08-28T12:00:00Z","status":"pending"}"#;
        let (url, server) = server(201, body);
        let pairing = client(url, Memory(Mutex::new(None)))
            .create_pairing("RockCast — test")
            .unwrap();
        assert_eq!(pairing.short_code, "AB12CD34");
        assert_eq!(pairing.device_display_name, "RockCast — test");
        assert_eq!(pairing.status, "pending");
        let request = server.join().unwrap();
        let request_lower = request.to_ascii_lowercase();
        assert!(request_lower.starts_with("post /api/v1/pairing-requests"));
        assert!(request.contains(r#""device_display_name":"RockCast — test""#));
        assert!(request.contains(r#""device_type":"windows""#));
        assert!(!request_lower.contains("\"device_name\""));
        assert!(!request_lower.contains("\"platform\""));
        assert!(!request_lower.contains("authorization:"));
    }

    #[test]
    fn pairing_link_keeps_the_approval_secret_out_of_the_query() {
        let pairing = PairingRequest {
            pairing_request_id: "request".into(),
            desktop_token: "desktop-token-1234".into(),
            approval_secret: "approval-secret-1234".into(),
            short_code: "AB12CD34".into(),
            verification_phrase: "AMBER-DAWN".into(),
            device_display_name: "RockCast — test".into(),
            device_type: "windows".into(),
            expires_at: "2026-08-28T12:00:00Z".into(),
            status: "pending".into(),
        };
        assert_eq!(
            pairing.deep_link("https://rockplatform.win/"),
            "https://rockplatform.win/?code=AB12CD34#secret=approval-secret-1234"
        );
    }

    #[test]
    fn completed_pairing_saves_only_to_secure_store_seam() {
        let body = r#"{"user_id":"must-not-be-used","device_id":"device","session_id":"session","access_token":"aaaaaaaaaaaaaaaa","access_expires_at":"2030-01-01T12:00:00Z","device_secret":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","account_display_name":"Alex's Rock account","device_display_name":"RockCast — test","device_type":"windows"}"#;
        let (url, server) = server(200, body);
        let store = Memory(Mutex::new(None));
        let pairing = PairingRequest {
            pairing_request_id: "request".into(),
            desktop_token: "desktop-token-1234".into(),
            approval_secret: "never-persist".into(),
            short_code: "AB12CD34".into(),
            verification_phrase: "AMBER-DAWN".into(),
            device_display_name: "RockCast — test".into(),
            device_type: "windows".into(),
            expires_at: "2026-08-28T12:00:00Z".into(),
            status: "pending".into(),
        };
        let account_client = client(url, store);
        let (profile, credentials) = account_client.complete_pairing_result(&pairing).unwrap();
        account_client
            .save_pairing_credentials(&credentials)
            .unwrap();
        assert_eq!(profile.device_id, "device");
        assert_eq!(profile.account_display_name, "Alex's Rock account");
        assert_eq!(profile.device_display_name, "RockCast — test");
        let request = server.join().unwrap();
        assert!(request.contains("/complete"));
        assert!(request.contains(r#"{"desktop_token":"desktop-token-1234"}"#));
        assert!(!request.contains("user_id"));
    }

    #[test]
    fn completion_payload_rejects_removed_user_id() {
        assert!(
            serde_json::from_str::<PairingCompletionRequest<'_>>(
                r#"{"desktop_token":"desktop-token-1234","user_id":"forbidden"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn pairing_poll_waits_for_browser_approval() {
        let (url, server) = server(202, "{}");
        let pairing = PairingRequest {
            pairing_request_id: "request".into(),
            desktop_token: "desktop-token-1234".into(),
            approval_secret: "never-persist".into(),
            short_code: "AB12CD34".into(),
            verification_phrase: "AMBER-DAWN".into(),
            device_display_name: "RockCast — test".into(),
            device_type: "windows".into(),
            expires_at: "2026-08-28T12:00:00Z".into(),
            status: "pending".into(),
        };
        assert!(matches!(
            client(url, Memory(Mutex::new(None))).complete_pairing_result(&pairing),
            Err(PairingPoll::Pending)
        ));
        assert!(server.join().unwrap().contains("/complete"));
    }

    #[test]
    fn pairing_poll_maps_terminal_server_states() {
        for (status, expected) in [
            (409, PairingPoll::DeviceLimit),
            (410, PairingPoll::Expired),
            (401, PairingPoll::Rejected),
        ] {
            let (url, server) = server(status, "{}");
            let pairing = PairingRequest {
                pairing_request_id: "request".into(),
                desktop_token: "desktop-token-1234".into(),
                approval_secret: "never-persist".into(),
                short_code: "AB12CD34".into(),
                verification_phrase: "AMBER-DAWN".into(),
                device_display_name: "RockCast — test".into(),
                device_type: "windows".into(),
                expires_at: "2026-08-28T12:00:00Z".into(),
                status: "pending".into(),
            };
            assert!(matches!(
                client(url, Memory(Mutex::new(None))).complete_pairing_result(&pairing),
                Err(actual) if actual == expected
            ));
            assert!(server.join().unwrap().contains("/complete"));
        }
    }

    #[test]
    fn device_session_reuses_the_durable_credential() {
        let (url, server) = server(
            200,
            r#"{"access_token":"cccccccccccccccc","access_expires_at":"2030-01-01T12:00:00Z"}"#,
        );
        let store = Memory(Mutex::new(Some(stored_credentials(
            "a".repeat(16).as_str(),
        ))));
        let client = client(url, store);
        let _guard = SESSION_MUTEX
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        client.issue_device_session_without_lock().unwrap();
        let credentials = client.store.load().unwrap().unwrap();
        assert_eq!(credentials.access_token(), "cccccccccccccccc");
        assert_eq!(credentials.device_secret(), "b".repeat(43));
        let request = server.join().unwrap();
        assert!(request.contains("/api/v1/auth/device-session"));
        assert!(request.contains(r#""device_id":"device""#));
        assert!(request.contains(r#""device_secret""#));
    }

    #[test]
    fn voice_access_token_reuses_fresh_session_without_renewal() {
        let store = Memory(Mutex::new(Some(stored_credentials("fresh-access-token"))));
        let client = client("http://127.0.0.1:9".into(), store);
        assert_eq!(
            client.voice_access_token().unwrap().as_deref(),
            Some("fresh-access-token")
        );
    }

    #[test]
    fn device_control_reuses_the_stored_pairing_identity() {
        let store = Memory(Mutex::new(Some(stored_credentials("fresh-access-token"))));
        let client = client("http://127.0.0.1:9".into(), store);
        assert_eq!(
            client
                .device_control_access_token(false)
                .unwrap()
                .as_deref(),
            Some("fresh-access-token")
        );
    }

    #[test]
    fn device_control_can_force_the_existing_session_renewal() {
        let (url, server) = server(
            200,
            r#"{"access_token":"cccccccccccccccc","access_expires_at":"2030-01-01T12:00:00Z"}"#,
        );
        let store = Memory(Mutex::new(Some(stored_credentials("fresh-access-token"))));
        let client = client(url, store);
        assert_eq!(
            client.device_control_access_token(true).unwrap().as_deref(),
            Some("cccccccccccccccc")
        );
        assert!(
            server
                .join()
                .unwrap()
                .contains("/api/v1/auth/device-session")
        );
    }

    #[test]
    fn voice_access_token_renews_when_expiry_is_near() {
        let (url, server) = server(
            200,
            r#"{"access_token":"cccccccccccccccc","access_expires_at":"2030-01-01T12:00:00Z"}"#,
        );
        let near_expiry = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
        let store = Memory(Mutex::new(Some(
            NativeCredentials::new("device".into(), "b".repeat(43), "a".repeat(16), near_expiry)
                .unwrap(),
        )));
        let client = client(url, store);
        assert_eq!(
            client.voice_access_token().unwrap().as_deref(),
            Some("cccccccccccccccc")
        );
        assert!(
            server
                .join()
                .unwrap()
                .contains("/api/v1/auth/device-session")
        );
    }

    #[test]
    fn unavailable_device_session_keeps_local_credentials() {
        let (url, server) = server(500, "{}");
        let store = Memory(Mutex::new(Some(stored_credentials(
            "a".repeat(16).as_str(),
        ))));
        let client = client(url, store);
        let _guard = SESSION_MUTEX
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(
            client.issue_device_session_without_lock().unwrap_err(),
            SessionError::Unavailable
        );
        assert!(client.store.load().unwrap().is_some());
        assert!(
            server
                .join()
                .unwrap()
                .contains("/api/v1/auth/device-session")
        );
    }

    #[test]
    fn invalid_device_credential_clears_local_credentials() {
        let (url, server) = server(401, r#"{"code":"device_credential_invalid"}"#);
        let store = Memory(Mutex::new(Some(stored_credentials(
            "a".repeat(16).as_str(),
        ))));
        let client = client(url, store);
        let _guard = SESSION_MUTEX
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(
            client.issue_device_session_without_lock().unwrap_err(),
            SessionError::Unauthorized
        );
        assert!(client.store.load().unwrap().is_none());
        assert!(
            server
                .join()
                .unwrap()
                .contains("/api/v1/auth/device-session")
        );
    }
}
