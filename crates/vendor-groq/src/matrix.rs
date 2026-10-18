//! Groq STT/TTS capability matrix from published docs.
//!
//! STT: <https://console.groq.com/docs/speech-to-text>
//! TTS: <https://console.groq.com/docs/text-to-speech>

use std::sync::LazyLock;

use node_webrtc_rust_voice_catalog as voice_catalog;

pub static STT_DOC: LazyLock<&'static str> =
    LazyLock::new(|| voice_catalog::stt_docs_url("groq").expect("groq stt docs in voice catalog"));
pub static TTS_DOC: LazyLock<&'static str> =
    LazyLock::new(|| voice_catalog::tts_docs_url("groq").expect("groq tts docs in voice catalog"));

/// TCP connect bound of the pooled HTTP client (`infra.unbounded-external-wait`).
pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Whole-request bound (headers + body) for TTS synthesis and the STT upload + answer. Groq had
/// no timeout before; 60 s matches the other REST vendors in this workspace.
pub const HTTP_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// How long an idle pooled connection is kept for the next utterance.
pub const POOL_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);
/// TCP keepalive on pooled connections.
pub const TCP_KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(30);
pub const DEFAULT_TRANSCRIPTIONS_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
pub const DEFAULT_SPEECH_URL: &str = "https://api.groq.com/openai/v1/audio/speech";

/// Documented transcription `model` values (Groq STT page).
pub static DOCUMENTED_STT_MODELS: LazyLock<&'static [&'static str]> =
    LazyLock::new(|| voice_catalog::stt_models("groq").expect("groq in voice catalog"));

/// Documented TTS `model` values (Groq TTS page).
pub static DOCUMENTED_TTS_MODELS: LazyLock<&'static [&'static str]> =
    LazyLock::new(|| voice_catalog::tts_models("groq").expect("groq in voice catalog"));

pub fn default_stt_model() -> &'static str {
    voice_catalog::default_stt_model("groq").expect("groq default STT in voice catalog")
}

pub fn default_tts_model() -> &'static str {
    voice_catalog::default_tts_model("groq").expect("groq default TTS in voice catalog")
}

pub fn validate_stt_model(model: &str) -> Result<(), String> {
    if DOCUMENTED_STT_MODELS.contains(&model) {
        return Ok(());
    }
    Err(format!(
        "unsupported Groq STT model `{model}`; documented models: {}",
        DOCUMENTED_STT_MODELS.join(", ")
    ))
}

pub fn validate_tts_model(model: &str) -> Result<(), String> {
    if DOCUMENTED_TTS_MODELS.contains(&model) {
        return Ok(());
    }
    Err(format!(
        "unsupported Groq TTS model `{model}`; documented models: {}",
        DOCUMENTED_TTS_MODELS.join(", ")
    ))
}

/// Parse `{ "text": "…" }` from `POST /audio/transcriptions` JSON response.
pub fn parse_transcription_json(raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let text = value.get("text")?.as_str()?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_stt_models_allowed() {
        for model in *DOCUMENTED_STT_MODELS {
            assert!(validate_stt_model(model).is_ok());
        }
    }

    #[test]
    fn unknown_stt_model_rejected() {
        assert!(validate_stt_model("whisper-1").is_err());
    }

    #[test]
    fn every_documented_stt_row() {
        for model in *DOCUMENTED_STT_MODELS {
            assert_eq!(validate_stt_model(model).unwrap(), ());
        }
        assert_eq!(DOCUMENTED_STT_MODELS.len(), 2);
    }

    #[test]
    fn documented_tts_models_allowed() {
        for model in *DOCUMENTED_TTS_MODELS {
            assert!(validate_tts_model(model).is_ok());
        }
    }

    #[test]
    fn unknown_tts_model_rejected() {
        assert!(validate_tts_model("tts-1").is_err());
    }

    #[test]
    fn parse_transcription_text() {
        let text = parse_transcription_json(r#"{"text":" hello "}"#).unwrap();
        assert_eq!(text, "hello");
    }
}

/// Validate `TtsConfig.transport` against the voice catalog (`None` / `auto` = the
/// catalog default). An unsupported value is a config error, raised at provider construction.
pub(crate) fn check_tts_transport(
    requested: Option<node_webrtc_rust_speech::config::SpeechTransport>,
) -> node_webrtc_rust_speech::error::SpeechResult<()> {
    node_webrtc_rust_speech::resolve_transport(
        "groq",
        "tts",
        requested,
        voice_catalog::tts_transports("groq"),
        voice_catalog::default_tts_transport("groq"),
    )
    .map(|_| ())
}

/// Validate `SttConfig.transport` against the voice catalog (`None` / `auto` = the
/// catalog default). An unsupported value is a config error, raised at provider construction.
pub(crate) fn check_stt_transport(
    requested: Option<node_webrtc_rust_speech::config::SpeechTransport>,
) -> node_webrtc_rust_speech::error::SpeechResult<()> {
    node_webrtc_rust_speech::resolve_transport(
        "groq",
        "stt",
        requested,
        voice_catalog::stt_transports("groq"),
        voice_catalog::default_stt_transport("groq"),
    )
    .map(|_| ())
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use node_webrtc_rust_speech::config::SpeechTransport;
    use node_webrtc_rust_speech::error::SpeechError;

    #[test]
    fn tts_transport_accepts_auto_and_http_and_rejects_websocket() {
        assert_eq!(voice_catalog::tts_transports("groq"), &["http"]);
        assert!(check_tts_transport(None).is_ok());
        assert!(check_tts_transport(Some(SpeechTransport::Auto)).is_ok());
        assert!(check_tts_transport(Some(SpeechTransport::Http)).is_ok());
        match check_tts_transport(Some(SpeechTransport::Websocket)) {
            Err(SpeechError::Config(message)) => assert_eq!(
                message,
                "groq tts transport 'websocket' is not supported; allowed: auto, http"
            ),
            other => panic!("expected a Config error, got {other:?}"),
        }
    }

    #[test]
    fn stt_transport_accepts_auto_and_http_and_rejects_websocket() {
        assert_eq!(voice_catalog::stt_transports("groq"), &["http"]);
        assert!(check_stt_transport(None).is_ok());
        assert!(check_stt_transport(Some(SpeechTransport::Auto)).is_ok());
        assert!(check_stt_transport(Some(SpeechTransport::Http)).is_ok());
        match check_stt_transport(Some(SpeechTransport::Websocket)) {
            Err(SpeechError::Config(message)) => assert_eq!(
                message,
                "groq stt transport 'websocket' is not supported; allowed: auto, http"
            ),
            other => panic!("expected a Config error, got {other:?}"),
        }
    }
}
