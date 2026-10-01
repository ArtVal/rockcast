//! Native account session client. Tokens never implement `Debug` or persistence formats.

mod client;
#[cfg(target_os = "linux")]
mod secret_service;
mod storage;
#[cfg(test)]
mod tests;
mod types;

pub(crate) use client::AccountClient;
pub use storage::{CredentialStore, OsCredentialStore};
pub use types::{
    AccountProfile, Device, NativeCredentials, PairingPoll, PairingRequest, SessionError,
};
pub(crate) use types::{PairingPollControl, pairing_poll_control};
