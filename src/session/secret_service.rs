//! Linux Secret Service backend for native session credential bytes.
//!
//! Stores one item in the default collection with attributes
//! `app_id=rockcast` and `kind=native-session`. Requires a running
//! `org.freedesktop.secrets` provider; otherwise callers see
//! `SecureStorageUnavailable`. Does not write plaintext files.

use std::collections::HashMap;

use secret_service::EncryptionType;
use secret_service::blocking::SecretService;

use super::types::SessionError;

const LABEL: &str = "RockCast session";
const CONTENT_TYPE: &str = "application/json";

fn attributes() -> HashMap<&'static str, &'static str> {
    HashMap::from([("app_id", "rockcast"), ("kind", "native-session")])
}

fn unavailable<E>(_: E) -> SessionError {
    SessionError::SecureStorageUnavailable
}

fn connect() -> Result<SecretService<'static>, SessionError> {
    SecretService::connect(EncryptionType::Dh).map_err(unavailable)
}

/// Reads the RockCast session secret, if any.
pub(super) fn load_bytes() -> Result<Option<Vec<u8>>, SessionError> {
    let service = connect()?;
    let found = service.search_items(attributes()).map_err(unavailable)?;
    let item = match found.unlocked.into_iter().next() {
        Some(item) => item,
        None => match found.locked.into_iter().next() {
            Some(item) => {
                service.unlock_all(&[&item]).map_err(unavailable)?;
                item
            }
            None => return Ok(None),
        },
    };
    item.get_secret().map(Some).map_err(unavailable)
}

/// Creates or replaces the RockCast session secret in the default collection.
pub(super) fn save_bytes(secret: &[u8]) -> Result<(), SessionError> {
    let service = connect()?;
    let collection = service.get_default_collection().map_err(unavailable)?;
    if collection.is_locked().map_err(unavailable)? {
        collection.unlock().map_err(unavailable)?;
    }
    collection
        .create_item(LABEL, attributes(), secret, true, CONTENT_TYPE)
        .map(|_| ())
        .map_err(unavailable)
}

/// Deletes every RockCast session item found by attributes.
pub(super) fn clear() -> Result<(), SessionError> {
    let service = connect()?;
    let found = service.search_items(attributes()).map_err(unavailable)?;
    if !found.locked.is_empty() {
        let locked: Vec<_> = found.locked.iter().collect();
        service.unlock_all(&locked).map_err(unavailable)?;
    }
    for item in found.unlocked.into_iter().chain(found.locked) {
        if item.is_locked().map_err(unavailable)? {
            item.unlock().map_err(unavailable)?;
        }
        item.delete().map_err(unavailable)?;
    }
    Ok(())
}
