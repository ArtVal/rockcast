//! Secure credential storage abstraction and OS-specific implementations.
//!
//! Windows uses DPAPI-protected `session.dpapi`. Linux uses Secret Service
//! (`session/secret_service.rs`). Other platforms fail closed.

#[cfg(windows)]
use std::{fs, io, path::PathBuf};

#[cfg(windows)]
use crate::settings;
use super::types::{LegacyStoredCredentials, NativeCredentials, SessionError, StoredCredentials};

/// Small seam for deterministic tests and unsupported OS handling.
pub trait CredentialStore: Send + Sync {
    fn load(&self) -> Result<Option<NativeCredentials>, SessionError>;
    fn save(&self, credentials: &NativeCredentials) -> Result<(), SessionError>;
    fn clear(&self) -> Result<(), SessionError>;
}

pub struct OsCredentialStore;

#[cfg(windows)]
impl OsCredentialStore {
    pub(super) fn path() -> Result<PathBuf, SessionError> {
        settings::app_dir()
            .map(|d| d.join("session.dpapi"))
            .ok_or(SessionError::SecureStorageUnavailable)
    }

    pub(super) fn recovery_path() -> Result<PathBuf, SessionError> {
        Ok(Self::path()?.with_extension("dpapi.recovery"))
    }
}

/// Decodes the current credential format and treats a successfully decrypted legacy refresh blob as absent.
pub(super) fn decode_credentials(plain: &[u8]) -> Result<Option<NativeCredentials>, SessionError> {
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

fn encode_credentials(credentials: &NativeCredentials) -> Result<Vec<u8>, SessionError> {
    serde_json::to_vec(&StoredCredentials {
        device_id: credentials.device_id.clone(),
        device_secret: credentials.device_secret.clone(),
        access_token: credentials.access_token.clone(),
        access_expires_at: credentials.access_expires_at.clone(),
    })
    .map_err(|_| SessionError::SecureStorageUnavailable)
}

fn credentials_match(left: &NativeCredentials, right: &NativeCredentials) -> bool {
    left.device_secret() == right.device_secret()
        && left.device_id() == right.device_id()
        && left.access_token() == right.access_token()
        && left.access_expires_at() == right.access_expires_at()
}

#[cfg(windows)]
pub(super) fn read_credentials(path: &PathBuf) -> Result<Option<NativeCredentials>, SessionError> {
    let encrypted = match fs::read(path) {
        Ok(value) => value,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(SessionError::SecureStorageUnavailable),
    };
    let plain = dpapi(false, &encrypted)?;
    decode_credentials(&plain)
}

#[cfg(windows)]
pub(super) fn write_credentials(
    path: &PathBuf,
    credentials: &NativeCredentials,
) -> Result<(), SessionError> {
    let plain = encode_credentials(credentials)?;
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
        if !credentials_match(&persisted, credentials) {
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
pub(super) fn dpapi(protect: bool, input: &[u8]) -> Result<Vec<u8>, SessionError> {
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

#[cfg(target_os = "linux")]
impl CredentialStore for OsCredentialStore {
    fn load(&self) -> Result<Option<NativeCredentials>, SessionError> {
        match super::secret_service::load_bytes()? {
            Some(bytes) => decode_credentials(&bytes),
            None => Ok(None),
        }
    }

    fn save(&self, credentials: &NativeCredentials) -> Result<(), SessionError> {
        let plain = encode_credentials(credentials)?;
        super::secret_service::save_bytes(&plain)?;
        let persisted = self.load()?.ok_or(SessionError::SecureStorageUnavailable)?;
        if !credentials_match(&persisted, credentials) {
            return Err(SessionError::SecureStorageUnavailable);
        }
        Ok(())
    }

    fn clear(&self) -> Result<(), SessionError> {
        super::secret_service::clear()
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
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
