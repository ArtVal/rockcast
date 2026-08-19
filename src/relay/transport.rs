#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TranscodeKind {
    Opus,
    AacAdts,
    Mp3,
    Symphonia,
}

#[derive(Clone)]
pub enum RelayTransport {
    Passthrough { content_type: String },
    WavPcm {
        kind: TranscodeKind,
        sample_rate: u32,
        channels: u16,
    },
}

pub fn choose_transport(preferred_content_type: &str, upstream_url: &str) -> RelayTransport {
    let ct = preferred_content_type.to_ascii_lowercase();
    let url = upstream_url.to_ascii_lowercase();
    if ct.contains("opus") || url.ends_with(".opus") || url.contains(".opus?") {
        RelayTransport::WavPcm {
            kind: TranscodeKind::Opus,
            sample_rate: 48_000,
            channels: 2,
        }
    } else if ct.contains("aac")
        || ct.contains("aacp")
        || ct.contains("x-aac")
        || ct.contains("he-aac")
        || url.contains("aacp")
        || url.contains(".aac")
        || url.contains("/aac")
    {
        RelayTransport::WavPcm {
            kind: TranscodeKind::AacAdts,
            sample_rate: 48_000,
            channels: 2,
        }
    } else if ct.contains("mpeg")
        || ct.contains("mp3")
        || url.ends_with(".mp3")
        || url.contains(".mp3?")
        || url.contains("/mp3")
    {
        RelayTransport::WavPcm {
            kind: TranscodeKind::Mp3,
            sample_rate: 48_000,
            channels: 2,
        }
    } else if ct.contains("ogg")
        || ct.contains("vorbis")
        || ct.contains("flac")
        || ct.contains("wav")
        || ct.contains("wave")
        || url.ends_with(".flac")
        || url.contains(".flac?")
        || url.contains("flac")
    {
        RelayTransport::WavPcm {
            kind: TranscodeKind::Symphonia,
            sample_rate: 48_000,
            channels: 2,
        }
    } else {
        RelayTransport::Passthrough {
            content_type: preferred_content_type.to_string(),
        }
    }
}

pub fn normalize_content_type(ct: &str) -> String {
    let normalized = ct.trim();
    let base = normalized.split(';').next().unwrap_or(normalized).trim();
    if base.is_empty() || base.eq_ignore_ascii_case("application/octet-stream") {
        return "audio/mpeg".into();
    }
    normalized.to_string()
}

pub fn tap_url_from_public(url: &str) -> String {
    url.replacen("/stream", "/tap", 1)
}
