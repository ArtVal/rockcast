use thiserror::Error;

use crate::cast::{channel::ChannelError, discovery::DiscoveryError};

#[derive(Debug, Error)]
pub enum CastError {
    #[error(transparent)]
    Discovery(#[from] DiscoveryError),
    #[error(transparent)]
    Channel(#[from] ChannelError),
}
