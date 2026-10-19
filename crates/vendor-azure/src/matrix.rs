//! Azure Speech capability matrix from published REST docs.
//!
//! STT short audio: <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-speech-to-text-short>
//! TTS REST: <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech>
//! Language + voice support (TTS ShortNames): <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/language-support?tabs=tts>
//! Regional voice list API: <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech#get-a-list-of-voices>

use std::sync::LazyLock;
use std::time::Duration;

use node_webrtc_rust_voice_catalog as voice_catalog;

/// Official Azure TTS ShortNames from shared `azure-tts-voices.json`.
pub static DOCUMENTED_TTS_VOICES: LazyLock<&'static [&'static str]> =
    LazyLock::new(voice_catalog::azure_tts_voices);

pub const STT_SHORT_AUDIO_DOC: &str =
    "https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-speech-to-text-short";
pub const TTS_REST_DOC: &str =
    "https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech";
pub const TTS_LANGUAGE_SUPPORT_DOC: &str =
    "https://learn.microsoft.com/en-us/azure/ai-services/speech-service/language-support?tabs=tts";
pub const TTS_VOICES_LIST_DOC: &str =
    "https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech#get-a-list-of-voices";

/// Documented REST recognition path segment (`…/recognition/{mode}/cognitiveservices/v1`).
///
/// Microsoft also documents a conversation **WebSocket URL** (`wss://…/stt/speech/recognition/conversation/…`)
/// and Voice Live realtime JSON (`wss://…/voice-live/realtime`). Official “how to recognize speech” directs
/// clients to the **Speech SDK**; the short-audio REST page is **finals only** and does **not** publish the
/// SDK binary/text frame protocol (`Path`, `X-RequestId`, `speech.hypothesis`, …) for custom raw clients.
/// This crate implements REST short audio only — no invented WebSocket frames.
pub static DOCUMENTED_STT_RECOGNITION_MODES: LazyLock<&'static [&'static str]> =
    LazyLock::new(|| voice_catalog::stt_models("azure").expect("azure in voice catalog"));

/// TCP connect bound of the pooled HTTP client (`infra.unbounded-external-wait`).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Whole-request bound (headers + body) for TTS synthesis and the STT upload + answer. Azure had
/// no timeout before; 60 s matches the other REST vendors in this workspace.
pub const HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long an idle pooled connection is kept for the next utterance.
pub const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// TCP keepalive on pooled connections.
pub const TCP_KEEPALIVE: Duration = Duration::from_secs(30);
/// TTS: time from the request to the first audio bytes (headers and any container header
/// included). A body that has not produced audio by then is an error.
pub const FIRST_AUDIO_TIMEOUT: Duration = Duration::from_secs(15);
/// TTS: longest silence between two body chunks once audio has started.
pub const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(10);

pub fn default_stt_mode() -> &'static str {
    voice_catalog::default_stt_model("azure").expect("azure default STT in voice catalog")
}

pub fn default_tts_voice() -> &'static str {
    voice_catalog::default_tts_voice("azure").expect("azure default TTS voice in voice catalog")
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
        "unsupported Azure TTS voice `{voice}`; must be a documented ShortName from language-support ({} voices); see {TTS_LANGUAGE_SUPPORT_DOC}",
        DOCUMENTED_TTS_VOICES.len()
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
    fn documented_voice_count_matches_snapshot() {
        assert_eq!(DOCUMENTED_TTS_VOICES.len(), 761);
        assert!(DOCUMENTED_TTS_VOICES.contains(&"en-US-JennyNeural"));
        assert!(DOCUMENTED_TTS_VOICES.contains(&"en-US-Ava:DragonHDLatestNeural"));
    }

    #[test]
    fn all_documented_voices_allowed() {
        for voice in *DOCUMENTED_TTS_VOICES {
            assert!(validate_tts_voice(voice).is_ok(), "voice: {voice}");
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

/// Validate `TtsConfig.transport` against the voice catalog (`None` / `auto` = the
/// catalog default). An unsupported value is a config error, raised at provider construction.
pub(crate) fn check_tts_transport(
    requested: Option<node_webrtc_rust_speech::config::SpeechTransport>,
) -> node_webrtc_rust_speech::error::SpeechResult<()> {
    node_webrtc_rust_speech::resolve_transport(
        "azure",
        "tts",
        requested,
        voice_catalog::tts_transports("azure"),
        voice_catalog::default_tts_transport("azure"),
    )
    .map(|_| ())
}

/// Validate `SttConfig.transport` against the voice catalog (`None` / `auto` = the
/// catalog default). An unsupported value is a config error, raised at provider construction.
pub(crate) fn check_stt_transport(
    requested: Option<node_webrtc_rust_speech::config::SpeechTransport>,
) -> node_webrtc_rust_speech::error::SpeechResult<()> {
    node_webrtc_rust_speech::resolve_transport(
        "azure",
        "stt",
        requested,
        voice_catalog::stt_transports("azure"),
        voice_catalog::default_stt_transport("azure"),
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
        assert_eq!(voice_catalog::tts_transports("azure"), &["http"]);
        assert!(check_tts_transport(None).is_ok());
        assert!(check_tts_transport(Some(SpeechTransport::Auto)).is_ok());
        assert!(check_tts_transport(Some(SpeechTransport::Http)).is_ok());
        match check_tts_transport(Some(SpeechTransport::Websocket)) {
            Err(SpeechError::Config(message)) => assert_eq!(
                message,
                "azure tts transport 'websocket' is not supported; allowed: auto, http"
            ),
            other => panic!("expected a Config error, got {other:?}"),
        }
    }

    #[test]
    fn stt_transport_accepts_auto_and_http_and_rejects_websocket() {
        assert_eq!(voice_catalog::stt_transports("azure"), &["http"]);
        assert!(check_stt_transport(None).is_ok());
        assert!(check_stt_transport(Some(SpeechTransport::Auto)).is_ok());
        assert!(check_stt_transport(Some(SpeechTransport::Http)).is_ok());
        match check_stt_transport(Some(SpeechTransport::Websocket)) {
            Err(SpeechError::Config(message)) => assert_eq!(
                message,
                "azure stt transport 'websocket' is not supported; allowed: auto, http"
            ),
            other => panic!("expected a Config error, got {other:?}"),
        }
    }
}
