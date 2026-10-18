//! Fish Audio realtime STT over the ElevenLabs-compatible socket, from published docs.
//!
//! Docs: <https://docs.fish.audio/developer-guide/compat/migrate-from-elevenlabs>
//!
//! - Socket: `{ws_base}/compat/elevenlabs/v1/speech-to-text/realtime`, header `xi-api-key`.
//! - Query: `model_id=fish-audio/<model>`, `commit_strategy=manual` (`vad` is refused at the
//!   handshake) and `audio_format=pcm_16000`. There is no `language_code`: Fish auto-detects.
//! - Client frames: `{"message_type":"input_audio_chunk","audio_base_64":…,"commit":bool,
//!   "sample_rate":16000}`; a commit is empty audio with `commit: true`.
//! - Server frames: `session_started`, `committed_transcript` (`text`) and
//!   `committed_transcript_with_timestamps` (ignored here). No partials.
#![cfg_attr(not(feature = "live"), allow(dead_code))]

use std::sync::LazyLock;
use std::time::Duration;

use node_webrtc_rust_speech::config::SpeechTransport;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::resolve_transport;
use node_webrtc_rust_voice_catalog as voice_catalog;
use serde::Deserialize;

use crate::matrix::DEFAULT_BASE_URL;

/// STT transports this crate implements. Lockstep with `voice_catalog::stt_transports("fishaudio")`.
pub fn implemented_stt_transports() -> &'static [&'static str] {
    &["websocket"]
}

/// Resolve `SttConfig.transport` against the voice catalog (`None` / `auto` = catalog default).
pub fn resolve_stt_transport(requested: Option<SpeechTransport>) -> SpeechResult<SpeechTransport> {
    resolve_transport(
        "fishaudio",
        "stt",
        requested,
        voice_catalog::stt_transports("fishaudio"),
        voice_catalog::default_stt_transport("fishaudio"),
    )?
    .ok_or_else(|| SpeechError::Config("fishaudio stt has no catalog default transport".into()))
}

/// Compat guide that documents the realtime STT socket.
pub static STT_DOC: LazyLock<&'static str> = LazyLock::new(|| {
    voice_catalog::stt_docs_url("fishaudio").expect("fishaudio stt docs in voice catalog")
});

pub static DOCUMENTED_STT_MODELS: LazyLock<&'static [&'static str]> =
    LazyLock::new(|| voice_catalog::stt_models("fishaudio").expect("fishaudio in voice catalog"));

pub fn default_stt_model() -> &'static str {
    voice_catalog::default_stt_model("fishaudio").expect("fishaudio default STT in voice catalog")
}

/// Alias for callers that expect a `DEFAULT_STT_MODEL` name.
pub static DEFAULT_STT_MODEL: LazyLock<&'static str> = LazyLock::new(default_stt_model);

/// PCM rate for `audio_format=pcm_16000` (the voice pipeline's STT input rate). A wrong
/// `audio_format` gives a confident wrong transcript instead of an error, so it is pinned.
pub const STT_PCM_SAMPLE_RATE: u32 = 16_000;

/// One connection lives at most 3600 s on Fish's side. Reconnect proactively at this
/// connection age, at a moment with nothing pending.
pub const STT_PROACTIVE_RECONNECT: Duration = Duration::from_secs(3300);
/// Bound for the polite close in `stop` and on reconnect; never waits for the server's answer.
pub const STT_CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

pub fn validate_stt_model(model: &str) -> Result<(), String> {
    if DOCUMENTED_STT_MODELS.contains(&model) {
        return Ok(());
    }
    Err(format!(
        "unsupported Fish Audio STT model `{model}`; documented realtime models: {}",
        DOCUMENTED_STT_MODELS.join(", ")
    ))
}

/// `{ws_base}/compat/elevenlabs/v1/speech-to-text/realtime?model_id=fish-audio/{model}&…`
/// with `https` mapped to `wss` and `http` to `ws`. `base` defaults to the public API origin.
pub fn realtime_stt_url(base: Option<&str>, model: &str) -> String {
    let base = base
        .map(str::trim)
        .filter(|base| !base.is_empty())
        .unwrap_or(DEFAULT_BASE_URL)
        .trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!(
        "{ws_base}/compat/elevenlabs/v1/speech-to-text/realtime?model_id=fish-audio/{model}&commit_strategy=manual&audio_format=pcm_16000"
    )
}

/// One client text frame. `audio_base_64` is standard base64 (no characters that need JSON
/// escaping) or empty for a commit.
pub fn input_audio_chunk_json(audio_base_64: &str, commit: bool) -> String {
    serde_json::json!({
        "message_type": "input_audio_chunk",
        "audio_base_64": audio_base_64,
        "commit": commit,
        "sample_rate": STT_PCM_SAMPLE_RATE,
    })
    .to_string()
}

/// A server text frame, reduced to what the provider acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMessage {
    SessionStarted,
    /// `committed_transcript` with its `text` (possibly empty).
    Committed(String),
    /// Any `message_type` ending in `error`; the text is the `message` or `error` string field,
    /// else the type name.
    Error(String),
    /// `committed_transcript_with_timestamps`, unknown types, malformed frames.
    Other,
}

#[derive(Deserialize)]
struct RawServerMessage {
    message_type: Option<String>,
    text: Option<String>,
    message: Option<String>,
    error: Option<String>,
}

pub fn parse_server_message(raw: &str) -> ServerMessage {
    let Ok(parsed) = serde_json::from_str::<RawServerMessage>(raw) else {
        return ServerMessage::Other;
    };
    let Some(kind) = parsed.message_type else {
        return ServerMessage::Other;
    };
    match kind.as_str() {
        "session_started" => ServerMessage::SessionStarted,
        "committed_transcript" => ServerMessage::Committed(parsed.text.unwrap_or_default()),
        kind if kind.ends_with("error") => ServerMessage::Error(
            parsed
                .message
                .or(parsed.error)
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| kind.to_string()),
        ),
        _ => ServerMessage::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implemented_transports_match_the_catalog() {
        let mut implemented = implemented_stt_transports().to_vec();
        let mut catalog = voice_catalog::stt_transports("fishaudio").to_vec();
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
                "fishaudio stt transport 'http' is not supported; allowed: auto, websocket"
            ),
            other => panic!("expected a Config error, got {other:?}"),
        }
    }

    #[test]
    fn default_model_is_transcribe_1() {
        assert_eq!(*DEFAULT_STT_MODEL, "transcribe-1");
        assert_eq!(*DOCUMENTED_STT_MODELS, &["transcribe-1"]);
        assert!(validate_stt_model("transcribe-1").is_ok());
    }

    #[test]
    fn undocumented_stt_models_are_rejected() {
        for model in ["transcribe-1-pro", "transcribe1", "scribe_v2_realtime", ""] {
            let err = validate_stt_model(model).unwrap_err();
            assert!(err.contains("transcribe-1"), "{err}");
        }
    }

    #[test]
    fn stt_doc_is_the_compat_guide() {
        assert_eq!(
            *STT_DOC,
            "https://docs.fish.audio/developer-guide/compat/migrate-from-elevenlabs"
        );
    }

    #[test]
    fn url_maps_schemes_and_pins_the_query() {
        let https = realtime_stt_url(None, "transcribe-1");
        assert_eq!(
            https,
            "wss://api.fish.audio/compat/elevenlabs/v1/speech-to-text/realtime?model_id=fish-audio/transcribe-1&commit_strategy=manual&audio_format=pcm_16000"
        );
        let http = realtime_stt_url(Some("http://127.0.0.1:9/"), "transcribe-1");
        assert!(
            http.starts_with("ws://127.0.0.1:9/compat/elevenlabs/"),
            "{http}"
        );
        let custom = realtime_stt_url(Some("https://example.test"), "transcribe-1");
        assert!(custom.starts_with("wss://example.test/compat/"), "{custom}");
        assert!(!https.contains("language_code"));
        // An empty endpoint falls back to the public origin.
        assert_eq!(realtime_stt_url(Some("  "), "transcribe-1"), https);
    }

    #[test]
    fn commit_and_audio_frames_have_the_documented_fields() {
        let audio: serde_json::Value =
            serde_json::from_str(&input_audio_chunk_json("AAEC", false)).unwrap();
        assert_eq!(
            audio,
            serde_json::json!({
                "message_type": "input_audio_chunk",
                "audio_base_64": "AAEC",
                "commit": false,
                "sample_rate": 16000,
            })
        );
        let commit: serde_json::Value =
            serde_json::from_str(&input_audio_chunk_json("", true)).unwrap();
        assert_eq!(commit["audio_base_64"], "");
        assert_eq!(commit["commit"], true);
    }

    #[test]
    fn server_messages_are_classified() {
        assert_eq!(
            parse_server_message(r#"{"message_type":"session_started","session_id":"s"}"#),
            ServerMessage::SessionStarted
        );
        assert_eq!(
            parse_server_message(r#"{"message_type":"committed_transcript","text":"hi there"}"#),
            ServerMessage::Committed("hi there".into())
        );
        assert_eq!(
            parse_server_message(r#"{"message_type":"committed_transcript"}"#),
            ServerMessage::Committed(String::new())
        );
        assert_eq!(
            parse_server_message(
                r#"{"message_type":"committed_transcript_with_timestamps","text":"hi","words":[]}"#
            ),
            ServerMessage::Other
        );
        assert_eq!(
            parse_server_message(r#"{"message_type":"input_error","message":"bad chunk"}"#),
            ServerMessage::Error("bad chunk".into())
        );
        assert_eq!(
            parse_server_message(r#"{"message_type":"error","error":"boom"}"#),
            ServerMessage::Error("boom".into())
        );
        assert_eq!(
            parse_server_message(r#"{"message_type":"auth_error"}"#),
            ServerMessage::Error("auth_error".into())
        );
        assert_eq!(
            parse_server_message(r#"{"message_type":"something_new"}"#),
            ServerMessage::Other
        );
        assert_eq!(parse_server_message("not json"), ServerMessage::Other);
        assert_eq!(
            parse_server_message(r#"{"text":"x"}"#),
            ServerMessage::Other
        );
    }
}
