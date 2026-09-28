//! Azure Speech capability matrix from published REST docs.
//!
//! STT short audio: <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-speech-to-text-short>
//! TTS REST: <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech>

pub const STT_SHORT_AUDIO_DOC: &str =
    "https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-speech-to-text-short";
pub const TTS_REST_DOC: &str =
    "https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech";

/// Documented REST recognition path segment (`…/recognition/{mode}/cognitiveservices/v1`).
/// Voice Live / SDK WebSocket partials are **not** implemented (no published raw frame spec in this crate).
pub const DOCUMENTED_STT_RECOGNITION_MODES: &[&str] = &["conversation"];

/// Sample neural voices from Language and voice support (REST `voice` element / SSML).
pub const DOCUMENTED_TTS_VOICES: &[&str] = &[
    "en-US-JennyNeural",
    "en-US-GuyNeural",
    "en-US-AriaNeural",
];

pub fn default_stt_mode() -> &'static str {
    "conversation"
}

pub fn default_tts_voice() -> &'static str {
    "en-US-JennyNeural"
}

pub fn validate_stt_mode(mode: &str) -> Result<(), String> {
    if DOCUMENTED_STT_RECOGNITION_MODES.contains(&mode) {
        return Ok(());
    }
    Err(format!(
        "unsupported Azure STT recognition mode `{mode}`; documented REST modes: {}",
        DOCUMENTED_STT_RECOGNITION_MODES.join(", ")
    ))
}

pub fn validate_tts_voice(voice: &str) -> Result<(), String> {
    if DOCUMENTED_TTS_VOICES.contains(&voice) {
        return Ok(());
    }
    Err(format!(
        "unsupported Azure TTS voice `{voice}`; documented sample voices: {}",
        DOCUMENTED_TTS_VOICES.join(", ")
    ))
}

/// Build short-audio STT URL for a resource host and BCP-47 language.
pub fn short_audio_stt_url(host: &str, mode: &str, language: &str) -> Result<String, String> {
    validate_stt_mode(mode)?;
    if language.is_empty() {
        return Err("Azure STT requires a language query parameter (e.g. en-US)".into());
    }
    let host = host.trim().trim_end_matches('/');
    let host = host
        .strip_prefix("https://")
        .or_else(|| host.strip_prefix("http://"))
        .unwrap_or(host);
    Ok(format!(
        "https://{host}/stt/speech/recognition/{mode}/cognitiveservices/v1?language={language}&format=simple"
    ))
}

/// Parse `DisplayText` from documented simple JSON response (final only — no partials on this REST API).
pub fn parse_short_audio_response(raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    if value.get("RecognitionStatus")?.as_str()? != "Success" {
        return None;
    }
    let text = value.get("DisplayText")?.as_str()?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

pub fn tts_rest_url(region: &str) -> String {
    let region = region.trim();
    format!("https://{region}.tts.speech.microsoft.com/cognitiveservices/v1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_mode_only() {
        assert!(validate_stt_mode("conversation").is_ok());
        assert!(validate_stt_mode("dictation").is_err());
    }

    #[test]
    fn sample_voices_allowed() {
        for voice in DOCUMENTED_TTS_VOICES {
            assert!(validate_tts_voice(voice).is_ok());
        }
    }

    #[test]
    fn unknown_voice_rejected() {
        assert!(validate_tts_voice("en-US-UnknownNeural").is_err());
    }

    #[test]
    fn short_audio_url_includes_language() {
        let url = short_audio_stt_url(
            "myresource.cognitiveservices.azure.com",
            "conversation",
            "en-US",
        )
        .unwrap();
        assert!(url.contains("language=en-US"));
        assert!(url.contains("recognition/conversation/"));
    }

    #[test]
    fn parse_simple_display_text() {
        let text = parse_short_audio_response(
            r#"{"RecognitionStatus":"Success","DisplayText":"Hello.","Offset":"0","Duration":"100"}"#,
        )
        .unwrap();
        assert_eq!(text, "Hello.");
    }
}
