//! Cast channel error types.

use thiserror::Error;

use super::super::proto::ProtoError;

#[derive(Debug, Error)]
pub enum ChannelError {
    #[error("TCP: {0}")]
    Io(#[from] std::io::Error),
    #[error("TLS: {0}")]
    Tls(String),
    #[error("protobuf: {0}")]
    Proto(#[from] ProtoError),
    #[error("message too large ({0} bytes)")]
    Oversized(u32),
    #[error("timed out waiting for Cast response")]
    Timeout,
    #[error("Cast operation cancelled")]
    Cancelled,
    #[error("{0}")]
    Msg(String),
}
