//! HTTP open + ICY reader setup for live streams.

use std::{
    io::Read,
    sync::{Arc, atomic::AtomicBool, mpsc},
    time::Duration,
};

use crate::{
    audio::format::read_format_peek,
    net::{metadata_interval, stream_client, stream_headers},
};

use super::super::icy::{IcyStreamReader, StopAwareBody, open_stream_response};

pub(super) type OpenedIcyReader = (String, Box<dyn Read + Send>, Vec<u8>);

pub(super) fn open_icy_reader(
    url: &str,
    stop: &Arc<AtomicBool>,
    title_tx: Option<mpsc::Sender<String>>,
    interruptible_body: bool,
) -> Result<OpenedIcyReader, String> {
    let headers = stream_headers(false);
    let client = stream_client(Duration::from_secs(10), None)?;
    let resp = open_stream_response(client, url, headers, stop)?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("audio/mpeg")
        .to_string();
    let meta_int = metadata_interval(resp.headers());
    log::info!("live decode HTTP ok content-type={content_type} icy-metaint={meta_int}");

    let stop_reader = Arc::clone(stop);
    let mut reader: Box<dyn Read + Send> = if interruptible_body {
        Box::new(IcyStreamReader::new(
            StopAwareBody::spawn(resp, Arc::clone(stop)),
            meta_int,
            stop_reader,
            title_tx,
        ))
    } else {
        Box::new(IcyStreamReader::new(resp, meta_int, stop_reader, title_tx))
    };

    let peek = read_format_peek(&mut reader, 8_192, stop)?;
    Ok((content_type, reader, peek))
}
