pub fn wav_response_headers(sample_rate: u32, channels: u16) -> String {
    let bits_per_sample = 16u16;
    let block_align = channels.saturating_mul(bits_per_sample / 8);
    let byte_rate = sample_rate.saturating_mul(u32::from(block_align));
    let content_length = 44u64 + u64::from(u32::MAX);
    format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: audio/wav\r\n\
         Content-Length: {content_length}\r\n\
         Cache-Control: no-cache, no-store\r\n\
         Pragma: no-cache\r\n\
         Connection: keep-alive\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Accept-Ranges: none\r\n\
         X-Audio-Sample-Rate: {sample_rate}\r\n\
         X-Audio-Channels: {channels}\r\n\
         X-Audio-Byte-Rate: {byte_rate}\r\n\
         \r\n"
    )
}

pub fn wav_live_header(sample_rate: u32, channels: u16) -> [u8; 44] {
    let bits_per_sample = 16u16;
    let block_align = channels * (bits_per_sample / 8);
    let byte_rate = sample_rate * u32::from(block_align);
    let mut out = [0u8; 44];
    out[0..4].copy_from_slice(b"RIFF");
    out[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    out[8..12].copy_from_slice(b"WAVE");
    out[12..16].copy_from_slice(b"fmt ");
    out[16..20].copy_from_slice(&16u32.to_le_bytes());
    out[20..22].copy_from_slice(&1u16.to_le_bytes());
    out[22..24].copy_from_slice(&channels.to_le_bytes());
    out[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    out[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    out[32..34].copy_from_slice(&block_align.to_le_bytes());
    out[34..36].copy_from_slice(&bits_per_sample.to_le_bytes());
    out[36..40].copy_from_slice(b"data");
    out[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
    out
}

pub fn stream_response_headers(content_type: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {content_type}\r\n\
         Cache-Control: no-cache, no-store\r\n\
         Pragma: no-cache\r\n\
         Connection: keep-alive\r\n\
         Transfer-Encoding: chunked\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Accept-Ranges: none\r\n\
         \r\n"
    )
}

pub fn tap_response_headers(content_type: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {content_type}\r\n\
         Cache-Control: no-cache, no-store\r\n\
         Pragma: no-cache\r\n\
         Connection: keep-alive\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Accept-Ranges: none\r\n\
         \r\n"
    )
}

pub fn pcm_tap_response_headers(sample_rate: u32, channels: u16) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: audio/L16\r\n\
         Cache-Control: no-cache, no-store\r\n\
         Pragma: no-cache\r\n\
         Connection: keep-alive\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Accept-Ranges: none\r\n\
         X-Audio-Sample-Rate: {sample_rate}\r\n\
         X-Audio-Channels: {channels}\r\n\
         \r\n"
    )
}
