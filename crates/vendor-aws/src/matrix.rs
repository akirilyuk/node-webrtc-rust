//! AWS Transcribe streaming + Polly matrices from published docs.
//!
//! Streaming setup: <https://docs.aws.amazon.com/transcribe/latest/dg/streaming-setting-up.html>
//! Polly SynthesizeSpeech: <https://docs.aws.amazon.com/polly/latest/dg/API_SynthesizeSpeech.html>

pub const TRANSCRIBE_STREAMING_DOC: &str =
    "https://docs.aws.amazon.com/transcribe/latest/dg/streaming-setting-up.html";
pub const POLLY_SYNTH_DOC: &str =
    "https://docs.aws.amazon.com/polly/latest/dg/API_SynthesizeSpeech.html";

use std::sync::LazyLock;

use node_webrtc_rust_voice_catalog as voice_catalog;

/// Language codes documented for streaming examples (config `model` holds the code; default `en-US`).
pub static DOCUMENTED_STT_LANGUAGE_CODES: LazyLock<&'static [&'static str]> =
    LazyLock::new(|| voice_catalog::stt_models("aws").expect("aws in voice catalog"));

/// Sample Polly neural voices (voice id in `TtsConfig.voice`; engine `neural` unless noted).
pub static DOCUMENTED_TTS_VOICES: LazyLock<&'static [&'static str]> =
    LazyLock::new(|| voice_catalog::tts_voices("aws").expect("aws in voice catalog"));

pub fn default_stt_language_code() -> &'static str {
    voice_catalog::default_stt_model("aws").expect("aws default STT in voice catalog")
}

pub fn default_tts_voice() -> &'static str {
    voice_catalog::default_tts_voice("aws").expect("aws default TTS voice in voice catalog")
}

pub fn validate_stt_language_code(code: &str) -> Result<(), String> {
    if DOCUMENTED_STT_LANGUAGE_CODES.contains(&code) {
        return Ok(());
    }
    Err(format!(
        "unsupported AWS Transcribe language code `{code}`; documented codes: {}",
        DOCUMENTED_STT_LANGUAGE_CODES.join(", ")
    ))
}

pub fn validate_tts_voice(voice: &str) -> Result<(), String> {
    if DOCUMENTED_TTS_VOICES.contains(&voice) {
        return Ok(());
    }
    Err(format!(
        "unsupported Polly voice `{voice}`; documented sample voices: {}",
        DOCUMENTED_TTS_VOICES.join(", ")
    ))
}

/// Parse documented `TranscriptEvent` JSON payload (IsPartial + Alternatives[0].Transcript).
#[cfg(test)]
pub fn parse_transcript_event_json(raw: &str) -> Option<(bool, String)> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let results = value.pointer("/Transcript/Results")?.as_array()?;
    let first = results.first()?;
    let is_partial = first
        .get("IsPartial")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let transcript = first
        .pointer("/Alternatives/0/Transcript")?
        .as_str()?
        .trim();
    if transcript.is_empty() {
        return None;
    }
    Some((is_partial, transcript.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_codes_matrix() {
        for code in *DOCUMENTED_STT_LANGUAGE_CODES {
            assert!(validate_stt_language_code(code).is_ok());
        }
        assert!(validate_stt_language_code("xx-XX").is_err());
    }

    #[test]
    fn polly_voices_matrix() {
        for voice in *DOCUMENTED_TTS_VOICES {
            assert!(validate_tts_voice(voice).is_ok());
        }
        assert!(validate_tts_voice("NotAVoice").is_err());
    }

    #[test]
    fn parse_partial_and_final() {
        let (partial, text) = parse_transcript_event_json(
            r#"{"Transcript":{"Results":[{"IsPartial":true,"Alternatives":[{"Transcript":"hel"}]}]}}"#,
        )
        .unwrap();
        assert!(partial);
        assert_eq!(text, "hel");
        let (fin, text) = parse_transcript_event_json(
            r#"{"Transcript":{"Results":[{"IsPartial":false,"Alternatives":[{"Transcript":"hello"}]}]}}"#,
        )
        .unwrap();
        assert!(!fin);
        assert_eq!(text, "hello");
    }
}

/// Validate `TtsConfig.transport` against the voice catalog (`None` / `auto` = the
/// catalog default). An unsupported value is a config error, raised at provider construction.
pub(crate) fn check_tts_transport(
    requested: Option<node_webrtc_rust_speech::config::SpeechTransport>,
) -> node_webrtc_rust_speech::error::SpeechResult<()> {
    node_webrtc_rust_speech::resolve_transport(
        "aws",
        "tts",
        requested,
        voice_catalog::tts_transports("aws"),
        voice_catalog::default_tts_transport("aws"),
    )
    .map(|_| ())
}

/// Validate `SttConfig.transport` against the voice catalog (`None` / `auto` = the
/// catalog default). An unsupported value is a config error, raised at provider construction.
pub(crate) fn check_stt_transport(
    requested: Option<node_webrtc_rust_speech::config::SpeechTransport>,
) -> node_webrtc_rust_speech::error::SpeechResult<()> {
    node_webrtc_rust_speech::resolve_transport(
        "aws",
        "stt",
        requested,
        voice_catalog::stt_transports("aws"),
        voice_catalog::default_stt_transport("aws"),
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
        assert_eq!(voice_catalog::tts_transports("aws"), &["http"]);
        assert!(check_tts_transport(None).is_ok());
        assert!(check_tts_transport(Some(SpeechTransport::Auto)).is_ok());
        assert!(check_tts_transport(Some(SpeechTransport::Http)).is_ok());
        match check_tts_transport(Some(SpeechTransport::Websocket)) {
            Err(SpeechError::Config(message)) => assert_eq!(
                message,
                "aws tts transport 'websocket' is not supported; allowed: auto, http"
            ),
            other => panic!("expected a Config error, got {other:?}"),
        }
    }

    #[test]
    fn stt_transport_accepts_auto_and_http_and_rejects_websocket() {
        assert_eq!(voice_catalog::stt_transports("aws"), &["http"]);
        assert!(check_stt_transport(None).is_ok());
        assert!(check_stt_transport(Some(SpeechTransport::Auto)).is_ok());
        assert!(check_stt_transport(Some(SpeechTransport::Http)).is_ok());
        match check_stt_transport(Some(SpeechTransport::Websocket)) {
            Err(SpeechError::Config(message)) => assert_eq!(
                message,
                "aws stt transport 'websocket' is not supported; allowed: auto, http"
            ),
            other => panic!("expected a Config error, got {other:?}"),
        }
    }
}
