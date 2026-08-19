use thiserror::Error;

#[derive(Debug, Error)]
pub enum RelayError {
    #[error("no LAN IPv4 to advertise to Cast (check Wi‑Fi)")]
    NoLanIp,
    #[error("bind relay socket: {0}")]
    Bind(String),
    #[error("upstream open: {0}")]
    Upstream(String),
    #[error("relay cancelled")]
    Cancelled,
}
