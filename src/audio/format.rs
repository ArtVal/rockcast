//! Stream format sniffing and ICY helpers.

use std::{
    io::{self, Read, Seek, SeekFrom},
    sync::atomic::{AtomicBool, Ordering},
};

use symphonia::core::{io::MediaSource, probe::Hint};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFormat {
    Mp3,
    AacAdts,
    OpusOgg,
    Ogg,
    Flac,
    Wav,
}

pub fn find_adts_sync(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(2)
        .position(|w| w[0] == 0xFF && (w[1] & 0xF6) == 0xF0)
}

pub fn find_mp3_sync(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|w| {
        let b0 = w[0];
        let b1 = w[1];
        b0 == 0xFF && (b1 & 0xE0) == 0xE0 && (b1 & 0x18) != 0x08
    })
}

pub fn infer_stream_format(url: &str, content_type: &str, peek: &[u8]) -> StreamFormat {
    let ct = content_type.to_ascii_lowercase();
    let url_l = url.to_ascii_lowercase();

    if ct.contains("opus") || url_l.ends_with(".opus") || url_l.contains(".opus?") {
        return StreamFormat::OpusOgg;
    }
    if ct.contains("flac") {
        return StreamFormat::Flac;
    }
    if ct.contains("wav") || ct.contains("wave") || ct.contains("pcm") {
        return StreamFormat::Wav;
    }
    if ct.contains("ogg") || ct.contains("vorbis") {
        return StreamFormat::Ogg;
    }
    if ct.contains("aac")
        || ct.contains("aacp")
        || ct.contains("x-aac")
        || ct.contains("he-aac")
        || url_l.contains("aacp")
        || url_l.contains(".aac")
        || url_l.contains("/aac")
    {
        return StreamFormat::AacAdts;
    }

    let adts = find_adts_sync(peek);
    let mp3 = find_mp3_sync(peek);
    match (adts, mp3) {
        (Some(a), Some(m)) if a <= m => StreamFormat::AacAdts,
        (Some(_), Some(_)) => StreamFormat::Mp3,
        (Some(_), None) => StreamFormat::AacAdts,
        (None, Some(_)) => StreamFormat::Mp3,
        (None, None) if ct.contains("mpeg") || ct.contains("mp3") => StreamFormat::Mp3,
        (None, None) => StreamFormat::Mp3,
    }
}

pub fn is_mp3_stream(url: &str, content_type: &str, peek: &[u8]) -> bool {
    infer_stream_format(url, content_type, peek) == StreamFormat::Mp3
}

pub fn apply_format_hint(hint: &mut Hint, format: StreamFormat) {
    match format {
        StreamFormat::Mp3 => {
            hint.with_extension("mp3");
        }
        StreamFormat::AacAdts => {
            hint.with_extension("adts");
        }
        StreamFormat::OpusOgg | StreamFormat::Ogg => {
            hint.with_extension("ogg");
        }
        StreamFormat::Flac => {
            hint.with_extension("flac");
        }
        StreamFormat::Wav => {
            hint.with_extension("wav");
        }
    }
}

pub fn apply_hint(hint: &mut Hint, url: &str, content_type: &str, peek: &[u8]) {
    apply_format_hint(hint, infer_stream_format(url, content_type, peek));
}

pub fn read_format_peek<R: Read>(
    reader: &mut R,
    max: usize,
    stop: &AtomicBool,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 1024];
    while out.len() < max {
        if stop.load(Ordering::SeqCst) {
            return Err("stopped".into());
        }
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(out)
}

pub struct PrefixedReader<R> {
    prefix: Vec<u8>,
    pos: usize,
    inner: R,
}

impl<R> PrefixedReader<R> {
    pub fn new(prefix: Vec<u8>, inner: R) -> Self {
        Self {
            prefix,
            pos: 0,
            inner,
        }
    }
}

impl<R: Read> Read for PrefixedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos < self.prefix.len() {
            let n = buf.len().min(self.prefix.len() - self.pos);
            buf[..n].copy_from_slice(&self.prefix[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        self.inner.read(buf)
    }
}

impl<R: Seek> Seek for PrefixedReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if self.pos < self.prefix.len() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "stream is not seekable",
            ));
        }
        self.inner.seek(pos)
    }
}

impl<R: MediaSource> MediaSource for PrefixedReader<R> {
    fn is_seekable(&self) -> bool {
        self.inner.is_seekable()
    }

    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len()
    }
}

pub fn parse_stream_title(meta: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(meta);
    let lower = text.to_ascii_lowercase();
    let start = lower.find("streamtitle='")? + "streamtitle='".len();
    let rest = &text[start..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::{StreamFormat, find_adts_sync, infer_stream_format, parse_stream_title};

    #[test]
    fn parses_icy_stream_title_case_insensitively() {
        assert_eq!(
            parse_stream_title(b"StreamTitle='Artist - Track';"),
            Some("Artist - Track".into())
        );
    }

    #[test]
    fn detects_aac_plus_from_adts_sync() {
        let mut adts = vec![0u8; 1200];
        adts[1198] = 0xFF;
        adts[1199] = 0xF1;
        assert_eq!(
            infer_stream_format("http://x/stream", "audio/mpeg", &adts),
            StreamFormat::AacAdts
        );
        assert_eq!(find_adts_sync(&adts), Some(1198));
    }

    #[test]
    fn prefers_aac_content_type_without_peek() {
        assert_eq!(
            infer_stream_format("http://x/stream", "audio/aacp", &[]),
            StreamFormat::AacAdts
        );
    }

    #[test]
    fn somafm_like_sync_with_audio_aac_content_type() {
        let peek = [0xFF, 0xF9, 0x5C, 0x80];
        assert_eq!(
            infer_stream_format("https://ice4.somafm.com/metal-128-aac", "audio/aac", &peek),
            StreamFormat::AacAdts
        );
    }
}
