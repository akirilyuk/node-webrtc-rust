//! Hardcoded ElevenLabs Scribe v2 Realtime matrix from published docs.
//!
//! WebSocket: `wss://api.elevenlabs.io/v1/speech-to-text/realtime`
//! Server events: `session_started`, `partial_transcript`, `committed_transcript`, …
//! Client audio: `input_audio_chunk` with `audio_base_64`, `commit`, `sample_rate`.

#![cfg_attr(not(feature = "live"), allow(dead_code))]

use std::sync::LazyLock;

use node_webrtc_rust_speech::config::SpeechTransport;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::resolve_transport;
use node_webrtc_rust_voice_catalog as voice_catalog;

/// STT transports this crate implements. Lockstep with `voice_catalog::stt_transports("elevenlabs")`.
pub fn implemented_stt_transports() -> &'static [&'static str] {
    &["websocket"]
}

/// Resolve `SttConfig.transport` against the voice catalog (`None` / `auto` = catalog default).
pub fn resolve_stt_transport(requested: Option<SpeechTransport>) -> SpeechResult<SpeechTransport> {
    resolve_transport(
        "elevenlabs",
        "stt",
        requested,
        voice_catalog::stt_transports("elevenlabs"),
        voice_catalog::default_stt_transport("elevenlabs"),
    )?
    .ok_or_else(|| SpeechError::Config("elevenlabs stt has no catalog default transport".into()))
}

pub static REALTIME_STT_DOC: LazyLock<&'static str> = LazyLock::new(|| {
    voice_catalog::stt_docs_url("elevenlabs").expect("elevenlabs stt docs in voice catalog")
});

pub static DOCUMENTED_STT_MODELS: LazyLock<&'static [&'static str]> =
    LazyLock::new(|| voice_catalog::stt_models("elevenlabs").expect("elevenlabs in voice catalog"));

pub fn default_stt_model() -> &'static str {
    voice_catalog::default_stt_model("elevenlabs").expect("elevenlabs default STT in voice catalog")
}

/// Alias for callers that expect a `DEFAULT_STT_MODEL` name.
pub static DEFAULT_STT_MODEL: LazyLock<&'static str> = LazyLock::new(default_stt_model);

/// PCM rate for `audio_format=pcm_16000` (matches voice pipeline STT input).
pub const STT_PCM_SAMPLE_RATE: u32 = 16_000;

pub fn validate_stt_model(model: &str) -> Result<(), String> {
    if DOCUMENTED_STT_MODELS.contains(&model) {
        return Ok(());
    }
    Err(format!(
        "unsupported ElevenLabs STT model `{model}`; documented realtime models: {}",
        DOCUMENTED_STT_MODELS.join(", ")
    ))
}

pub fn realtime_websocket_url(model: &str, language: Option<&str>) -> Result<String, String> {
    realtime_websocket_url_at(None, model, language)
}

/// Same as [`realtime_websocket_url`] against `base` (`SttConfig.endpoint`, an `http(s)://` or
/// `ws(s)://` origin); `None` is the ElevenLabs API.
pub fn realtime_websocket_url_at(
    base: Option<&str>,
    model: &str,
    language: Option<&str>,
) -> Result<String, String> {
    validate_stt_model(model)?;
    let base = base
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .unwrap_or(crate::matrix::DEFAULT_BASE_URL)
        .trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    let mut url = format!(
        "{ws_base}/v1/speech-to-text/realtime?model_id={model}&commit_strategy=manual&audio_format=pcm_16000"
    );
    if let Some(lang) = language.filter(|l| !l.is_empty()) {
        url.push_str("&language_code=");
        url.push_str(lang);
    }
    Ok(url)
}

pub fn input_audio_chunk_json(audio_base_64: &str, commit: bool) -> String {
    serde_json::json!({
        "message_type": "input_audio_chunk",
        "audio_base_64": audio_base_64,
        "commit": commit,
        "sample_rate": STT_PCM_SAMPLE_RATE,
    })
    .to_string()
}

/// Server `message_type` values the API reference documents as errors.
const ERROR_TYPES: &[&str] = &[
    "error",
    "auth_error",
    "quota_exceeded",
    "commit_throttled",
    "unaccepted_terms",
    "rate_limited",
    "queue_overflow",
    "resource_exhausted",
    "session_time_limit_exceeded",
    "input_error",
    "invalid_request",
    "chunk_size_exceeded",
    "insufficient_audio_activity",
    "transcriber_error",
];

/// A server text frame, reduced to what the provider acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMessage {
    /// `partial_transcript`: interim text (may be empty).
    Partial(String),
    /// `committed_transcript`: the final text of everything since the last commit (may be empty).
    Committed(String),
    /// A documented error `message_type`; `message` is its `error` field, else the type name.
    /// `answers_commit` for `commit_throttled`, the one error that is about a commit.
    Error {
        answers_commit: bool,
        message: String,
    },
    /// Everything else (`session_started`, `warning`, unknown types, malformed frames).
    Other,
}

pub fn parse_server_message(raw: &str) -> ServerMessage {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return ServerMessage::Other;
    };
    let Some(kind) = value.get("message_type").and_then(|v| v.as_str()) else {
        return ServerMessage::Other;
    };
    let text = || {
        value
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    match kind {
        "partial_transcript" => ServerMessage::Partial(text()),
        "committed_transcript" => ServerMessage::Committed(text()),
        kind if ERROR_TYPES.contains(&kind) => ServerMessage::Error {
            answers_commit: kind == "commit_throttled",
            message: value
                .get("error")
                .and_then(|v| v.as_str())
                .filter(|e| !e.is_empty())
                .unwrap_or(kind)
                .to_string(),
        },
        _ => ServerMessage::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implemented_transports_match_the_catalog() {
        let mut implemented = implemented_stt_transports().to_vec();
        let mut catalog = voice_catalog::stt_transports("elevenlabs").to_vec();
        implemented.sort_unstable();
        catalog.sort_unstable();
        assert_eq!(implemented, catalog);
    }

    #[test]
    fn stt_transport_auto_is_websocket_and_http_is_rejected() {
        assert_eq!(
            resolve_stt_transport(None).unwrap(),
            SpeechTransport::Websocket
        );
        match resolve_stt_transport(Some(SpeechTransport::Http)) {
            Err(SpeechError::Config(message)) => assert_eq!(
                message,
                "elevenlabs stt transport 'http' is not supported; allowed: auto, websocket"
            ),
            other => panic!("expected a Config error, got {other:?}"),
        }
    }

    #[test]
    fn only_scribe_v2_realtime_allowed() {
        assert!(validate_stt_model("scribe_v2_realtime").is_ok());
        assert!(validate_stt_model("scribe_v2").is_err());
    }

    #[test]
    fn server_messages_are_classified() {
        assert_eq!(
            parse_server_message(r#"{"message_type":"partial_transcript","text":" hel "}"#),
            ServerMessage::Partial("hel".into())
        );
        assert_eq!(
            parse_server_message(r#"{"message_type":"committed_transcript","text":"hello"}"#),
            ServerMessage::Committed("hello".into())
        );
        assert_eq!(
            parse_server_message(r#"{"message_type":"committed_transcript"}"#),
            ServerMessage::Committed(String::new())
        );
        assert_eq!(
            parse_server_message(r#"{"message_type":"commit_throttled","error":"slow down"}"#),
            ServerMessage::Error {
                answers_commit: true,
                message: "slow down".into()
            }
        );
        assert_eq!(
            parse_server_message(r#"{"message_type":"quota_exceeded"}"#),
            ServerMessage::Error {
                answers_commit: false,
                message: "quota_exceeded".into()
            }
        );
        for other in [
            r#"{"message_type":"session_started","session_id":"s"}"#,
            r#"{"message_type":"warning","warning":"w"}"#,
            "not json",
        ] {
            assert_eq!(parse_server_message(other), ServerMessage::Other);
        }
    }

    #[test]
    fn url_follows_the_endpoint_override() {
        let url =
            realtime_websocket_url_at(Some("http://127.0.0.1:9/"), "scribe_v2_realtime", None)
                .unwrap();
        assert!(url.starts_with("ws://127.0.0.1:9/v1/speech-to-text/realtime?model_id="));
    }

    #[test]
    fn commit_chunk_shape() {
        let msg = input_audio_chunk_json("", true);
        assert!(msg.contains("\"commit\":true"));
        assert!(msg.contains("input_audio_chunk"));
    }
}
