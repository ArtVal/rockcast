use thiserror::Error;

#[derive(Debug, Error)]
pub enum LocalError {
    #[error("audio: {0}")]
    Audio(String),
    #[error("stream: {0}")]
    Stream(String),
}
