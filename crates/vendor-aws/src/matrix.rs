//! AWS Transcribe streaming + Polly matrices from published docs.
//!
//! Streaming setup: <https://docs.aws.amazon.com/transcribe/latest/dg/streaming-setting-up.html>
//! Polly SynthesizeSpeech: <https://docs.aws.amazon.com/polly/latest/dg/API_SynthesizeSpeech.html>

pub const TRANSCRIBE_STREAMING_DOC: &str =
    "https://docs.aws.amazon.com/transcribe/latest/dg/streaming-setting-up.html";
pub const POLLY_SYNTH_DOC: &str =
    "https://docs.aws.amazon.com/polly/latest/dg/API_SynthesizeSpeech.html";

/// Language codes documented for streaming examples (config `model` holds the code; default `en-US`).
pub const DOCUMENTED_STT_LANGUAGE_CODES: &[&str] = &[
    "en-US", "es-US", "fr-FR", "de-DE", "pt-BR", "ja-JP", "ko-KR", "zh-CN", "it-IT", "hi-IN",
];

/// Sample Polly neural voices (voice id in `TtsConfig.voice`; engine `neural` unless noted).
pub const DOCUMENTED_TTS_VOICES: &[&str] = &["Joanna", "Matthew", "Amy", "Brian", "Ruth", "Stephen"];

pub fn default_stt_language_code() -> &'static str {
    "en-US"
}

pub fn default_tts_voice() -> &'static str {
    "Joanna"
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
pub fn parse_transcript_event_json(raw: &str) -> Option<(bool, String)> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let results = value.pointer("/Transcript/Results")?.as_array()?;
    let first = results.first()?;
    let is_partial = first.get("IsPartial").and_then(|v| v.as_bool()).unwrap_or(false);
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
        for code in DOCUMENTED_STT_LANGUAGE_CODES {
            assert!(validate_stt_language_code(code).is_ok());
        }
        assert!(validate_stt_language_code("xx-XX").is_err());
    }

    #[test]
    fn polly_voices_matrix() {
        for voice in DOCUMENTED_TTS_VOICES {
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
