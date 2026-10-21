//! Mistral Voxtral realtime STT capability matrix from the published docs.
//!
//! Realtime transcription: <https://docs.mistral.ai/studio/audio/speech-to-text/transcribe-in-realtime>
//!
//! Client messages: `session.update`, `input_audio.append`, `input_audio.end`. Server messages
//! read here: `transcription.text.delta`, `transcription.done`, `error`. Everything else
//! (`session.created`, `session.updated`, `transcription.language`, ...) is ignored.

use std::time::Duration;

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use serde_json::{json, Value};

pub const STT_DOC: &str =
    "https://docs.mistral.ai/studio/audio/speech-to-text/transcribe-in-realtime";

pub const DOCUMENTED_STT_MODELS: &[&str] = &[
    "voxtral-mini-transcribe-realtime-3-0",
    "voxtral-mini-transcribe-realtime-3",
];
pub const DEFAULT_STT_MODEL: &str = "voxtral-mini-transcribe-realtime-3-0";

/// The speech pipeline hands STT providers 16 kHz mono s16le (as for the Fish vendor).
pub const STT_SAMPLE_RATE: u32 = 16_000;

/// `target_streaming_delay_ms` we send: the Mistral voice-agent SDK default for agents. The
/// docs recommend 480 for accuracy.
pub const STREAMING_DELAY_MS: u32 = 240;

/// Appends carry at least this much audio (100 ms of 16 kHz s16 mono, the docs' sample chunk
/// size), except the flush at the end of an utterance.
pub const APPEND_TARGET_BYTES: usize = 3200;
/// Documented maximum of one `input_audio.append` (decoded bytes).
pub const APPEND_MAX_BYTES: usize = 262_144;

/// Connect, the `session.update` write, and every other socket write.
pub const STT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// `input_audio.end` to `transcription.done`. Shorter than the agent's 10 s finalize wait, so
/// the named timeout error arrives first.
pub const STT_DONE_TIMEOUT: Duration = Duration::from_secs(8);
/// A polite close frame; the server's answer is never awaited.
pub const STT_CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// Bounds for one STT provider. [`SttTimeouts::default`] holds the constants above; tests
/// shorten them.
#[derive(Debug, Clone, Copy)]
pub struct SttTimeouts {
    pub connect: Duration,
    pub done: Duration,
    pub close: Duration,
}

impl Default for SttTimeouts {
    fn default() -> Self {
        Self {
            connect: STT_CONNECT_TIMEOUT,
            done: STT_DONE_TIMEOUT,
            close: STT_CLOSE_TIMEOUT,
        }
    }
}

/// An unknown id is rejected before anything is sent.
pub fn validate_stt_model(model: &str) -> SpeechResult<()> {
    if DOCUMENTED_STT_MODELS.contains(&model) {
        return Ok(());
    }
    Err(SpeechError::Config(format!(
        "unsupported Mistral STT model `{model}`; documented models: {}",
        DOCUMENTED_STT_MODELS.join(", ")
    )))
}

/// Documented range: 80 to 1200 ms, or 2400 ms.
pub fn validate_streaming_delay(ms: u32) -> SpeechResult<()> {
    if (80..=1200).contains(&ms) || ms == 2400 {
        return Ok(());
    }
    Err(SpeechError::Config(format!(
        "unsupported Mistral target_streaming_delay_ms {ms}; allowed: 80 to 1200, or 2400"
    )))
}

/// `wss://api.mistral.ai/v1/audio/transcriptions/realtime?model=<id>`; `https` maps to `wss`
/// and `http` to `ws` for a custom base (the mock server in tests).
pub fn realtime_stt_url(base: Option<&str>, model: &str) -> String {
    let base = base
        .filter(|b| !b.trim().is_empty())
        .unwrap_or(crate::matrix::DEFAULT_BASE_URL)
        .trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{ws_base}/v1/audio/transcriptions/realtime?model={model}")
}

/// `language` only for an exactly-two-letter ISO 639-1 code; any other tag (`en-US`, `multi`)
/// is left out so Mistral auto-detects.
pub fn session_language(language: Option<&str>) -> Option<String> {
    let language = language?.trim();
    (language.len() == 2 && language.chars().all(|c| c.is_ascii_alphabetic()))
        .then(|| language.to_ascii_lowercase())
}

pub(crate) fn session_update_json(language: Option<&str>) -> String {
    let mut session = json!({
        "audio_format": {"encoding": "pcm_s16le", "sample_rate": STT_SAMPLE_RATE},
        "target_streaming_delay_ms": STREAMING_DELAY_MS,
    });
    if let Some(language) = session_language(language) {
        session["language"] = Value::String(language);
    }
    json!({"type": "session.update", "session": session}).to_string()
}

pub(crate) fn append_json(audio_base64: &str) -> String {
    json!({"type": "input_audio.append", "audio": audio_base64}).to_string()
}

pub(crate) const END_JSON: &str = r#"{"type":"input_audio.end"}"#;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ServerEvent {
    Delta(String),
    /// `transcription.done`; `None` when the event carries no `text`.
    Done(Option<String>),
    Error(String),
    Other,
}

pub(crate) fn parse_server_event(text: &str) -> ServerEvent {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return ServerEvent::Other;
    };
    match value.get("type").and_then(Value::as_str) {
        Some("transcription.text.delta") => ServerEvent::Delta(
            value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        Some("transcription.done") => ServerEvent::Done(
            value
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string),
        ),
        Some("error") => {
            let error = value.get("error");
            let code = match error.and_then(|e| e.get("code")) {
                Some(Value::String(code)) => code.clone(),
                Some(Value::Number(code)) => code.to_string(),
                _ => "unknown".to_string(),
            };
            let message = match error.and_then(|e| e.get("message")) {
                Some(Value::String(message)) => message.clone(),
                Some(other) => other.to_string(),
                None => String::new(),
            };
            ServerEvent::Error(format!(
                "mistral stt error {code}: {}",
                message.chars().take(300).collect::<String>()
            ))
        }
        _ => ServerEvent::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_validation_matches_the_documented_range() {
        for ok in [80, 240, 480, 1200, 2400] {
            assert!(validate_streaming_delay(ok).is_ok(), "{ok}");
        }
        for bad in [0, 79, 1201, 2000, 2401] {
            assert!(validate_streaming_delay(bad).is_err(), "{bad}");
        }
        assert!(validate_streaming_delay(STREAMING_DELAY_MS).is_ok());
    }

    #[test]
    fn model_validation_names_the_documented_ids() {
        for model in DOCUMENTED_STT_MODELS {
            assert!(validate_stt_model(model).is_ok());
        }
        let err = validate_stt_model("voxtral-mini-transcribe-realtime-latest")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("voxtral-mini-transcribe-realtime-3-0"),
            "{err}"
        );
        assert!(DOCUMENTED_STT_MODELS.contains(&DEFAULT_STT_MODEL));
    }

    #[test]
    fn url_building() {
        assert_eq!(
            realtime_stt_url(None, DEFAULT_STT_MODEL),
            "wss://api.mistral.ai/v1/audio/transcriptions/realtime?model=voxtral-mini-transcribe-realtime-3-0"
        );
        assert_eq!(
            realtime_stt_url(Some("https://example.test/"), "m"),
            "wss://example.test/v1/audio/transcriptions/realtime?model=m"
        );
        assert_eq!(
            realtime_stt_url(Some("http://127.0.0.1:9"), "m"),
            "ws://127.0.0.1:9/v1/audio/transcriptions/realtime?model=m"
        );
    }

    #[test]
    fn language_is_sent_only_as_a_two_letter_code() {
        assert_eq!(session_language(Some("en")).as_deref(), Some("en"));
        assert_eq!(session_language(Some("DE")).as_deref(), Some("de"));
        for other in [None, Some(""), Some("en-US"), Some("multi"), Some("e1")] {
            assert_eq!(session_language(other), None, "{other:?}");
        }
    }

    #[test]
    fn server_events_parse() {
        assert_eq!(
            parse_server_event(r#"{"type":"transcription.text.delta","text":" hi"}"#),
            ServerEvent::Delta(" hi".into())
        );
        assert_eq!(
            parse_server_event(r#"{"type":"transcription.done","text":"hi","language":"en"}"#),
            ServerEvent::Done(Some("hi".into()))
        );
        assert_eq!(
            parse_server_event(r#"{"type":"error","error":{"message":"bad","code":429}}"#),
            ServerEvent::Error("mistral stt error 429: bad".into())
        );
        assert_eq!(
            parse_server_event(r#"{"type":"session.created"}"#),
            ServerEvent::Other
        );
        assert_eq!(parse_server_event("not json"), ServerEvent::Other);
    }
}
