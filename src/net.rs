//! Shared HTTP policy for radio streams.

use std::time::Duration;

use reqwest::{
    blocking::Client,
    header::{HeaderMap, HeaderValue},
};

pub fn stream_headers(connection_close: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("Icy-MetaData", HeaderValue::from_static("1"));
    headers.insert("Accept", HeaderValue::from_static("*/*"));
    if connection_close {
        headers.insert("Connection", HeaderValue::from_static("close"));
    }
    headers
}

pub fn stream_client(
    connect_timeout: Duration,
    overall_timeout: Option<Duration>,
) -> Result<Client, String> {
    let mut builder = Client::builder()
        .user_agent("RockCast/0.1")
        .connect_timeout(connect_timeout);
    builder = match overall_timeout {
        Some(timeout) => builder.timeout(timeout),
        None => builder.timeout(None),
    };
    builder.build().map_err(|e| e.to_string())
}

pub fn metadata_interval(headers: &HeaderMap) -> usize {
    headers
        .get("icy-metaint")
        .or_else(|| headers.get("Icy-MetaInt"))
        .or_else(|| headers.get("ice-metaint"))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_header_is_case_tolerant() {
        let mut headers = HeaderMap::new();
        headers.insert("ice-metaint", HeaderValue::from_static("16000"));
        assert_eq!(metadata_interval(&headers), 16_000);
    }
}
