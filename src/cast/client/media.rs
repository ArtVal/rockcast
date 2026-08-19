//! Cast LOAD helpers and content-type negotiation.

use super::super::channel::ChannelError;
use super::session::LoadProgress;

pub(super) fn candidate_content_types(content_type: &str) -> Vec<String> {
    let mut out = Vec::new();
    let normalized = content_type.trim().to_ascii_lowercase();
    let mut push = |value: &str| {
        if !value.is_empty() && !out.iter().any(|existing| existing == value) {
            out.push(value.to_string());
        }
    };

    push(&normalized);
    if normalized.contains("opus") {
        push("audio/ogg; codecs=opus");
        push("audio/ogg");
        push("application/ogg");
        push("audio/opus");
    } else if normalized.contains("vorbis") || normalized == "audio/ogg" || normalized == "application/ogg"
    {
        push("audio/ogg; codecs=vorbis");
        push("audio/ogg");
        push("application/ogg");
    }
    if normalized != "audio/mpeg" {
        push("audio/mpeg");
    }
    out
}

pub(super) fn classify_media_status(
    v: &serde_json::Value,
    req: u32,
    url: &str,
) -> Result<LoadProgress, ChannelError> {
    let rid = v.get("requestId").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    let status = v
        .get("status")
        .and_then(|s| s.as_array())
        .and_then(|a| a.first());
    let Some(st) = status else {
        return if rid == req {
            Ok(LoadProgress::Ready(None))
        } else {
            Ok(LoadProgress::Pending)
        };
    };

    let content_ok = st
        .pointer("/media/contentId")
        .and_then(|c| c.as_str())
        .is_some_and(|c| c == url);
    if rid != req && !content_ok {
        return Ok(LoadProgress::Pending);
    }

    let state = st
        .get("playerState")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let idle_reason = st
        .get("idleReason")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let mid = st.get("mediaSessionId").and_then(|m| m.as_i64());
    log::info!(
        "Cast LOAD status: state={} idleReason={} content_ok={} requestId={}",
        state,
        idle_reason,
        content_ok,
        rid
    );

    match state {
        "BUFFERING" | "PLAYING" | "PAUSED" => Ok(LoadProgress::Ready(mid)),
        "IDLE" if matches!(idle_reason, "ERROR" | "CANCELLED" | "INTERRUPTED" | "FINISHED") => {
            Err(ChannelError::Msg(format!(
                "Cast LOAD stalled in IDLE ({idle_reason})"
            )))
        }
        "IDLE" | "" => Ok(LoadProgress::Pending),
        _ => Ok(LoadProgress::Pending),
    }
}

#[cfg(test)]
mod tests {
    use super::{candidate_content_types, classify_media_status};
    use super::super::session::LoadProgress;
    use serde_json::json;

    #[test]
    fn opus_candidates_include_ogg_variants() {
        let values = candidate_content_types("audio/ogg; codecs=opus");
        assert!(values.iter().any(|v| v == "audio/ogg; codecs=opus"));
        assert!(values.iter().any(|v| v == "audio/ogg"));
        assert!(values.iter().any(|v| v == "application/ogg"));
        assert!(values.iter().any(|v| v == "audio/opus"));
    }

    #[test]
    fn idle_status_is_not_treated_as_success() {
        let payload = json!({
            "type": "MEDIA_STATUS",
            "requestId": 7,
            "status": [{
                "mediaSessionId": 2,
                "playerState": "IDLE",
                "media": { "contentId": "http://127.0.0.1/stream" }
            }]
        });
        assert_eq!(
            classify_media_status(&payload, 7, "http://127.0.0.1/stream").unwrap(),
            LoadProgress::Pending
        );
    }

    #[test]
    fn buffering_status_is_treated_as_success() {
        let payload = json!({
            "type": "MEDIA_STATUS",
            "requestId": 7,
            "status": [{
                "mediaSessionId": 2,
                "playerState": "BUFFERING",
                "media": { "contentId": "http://127.0.0.1/stream" }
            }]
        });
        assert_eq!(
            classify_media_status(&payload, 7, "http://127.0.0.1/stream").unwrap(),
            LoadProgress::Ready(Some(2))
        );
    }
}
