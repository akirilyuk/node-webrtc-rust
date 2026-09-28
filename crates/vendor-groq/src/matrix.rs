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
