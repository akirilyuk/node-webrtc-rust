//! Hardcoded ElevenLabs Scribe v2 Realtime matrix from published docs.
//!
//! WebSocket: `wss://api.elevenlabs.io/v1/speech-to-text/realtime`
//! Server events: `session_started`, `partial_transcript`, `committed_transcript`, …
//! Client audio: `input_audio_chunk` with `audio_base_64`, `commit`, `sample_rate`.

pub const REALTIME_STT_DOC: &str =
    "https://elevenlabs.io/docs/api-reference/speech-to-text/v-1-speech-to-text-realtime";

pub const DEFAULT_STT_MODEL: &str = "scribe_v2_realtime";

pub fn default_stt_model() -> &'static str {
    DEFAULT_STT_MODEL
}

/// PCM rate for `audio_format=pcm_16000` (matches voice pipeline STT input).
pub const STT_PCM_SAMPLE_RATE: u32 = 16_000;

pub fn validate_stt_model(model: &str) -> Result<(), String> {
    if model == DEFAULT_STT_MODEL {
        return Ok(());
    }
    Err(format!(
        "unsupported ElevenLabs STT model `{model}`; documented realtime model: {DEFAULT_STT_MODEL}"
    ))
}

pub fn realtime_websocket_url(model: &str, language: Option<&str>) -> Result<String, String> {
    validate_stt_model(model)?;
    let mut url = format!(
        "wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id={model}&commit_strategy=manual&audio_format=pcm_16000"
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

pub fn parse_realtime_message(raw: &str) -> Option<node_webrtc_rust_speech::pipeline::SttTranscript> {
    use node_webrtc_rust_speech::pipeline::SttTranscript;
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    match value.get("message_type")?.as_str()? {
        "partial_transcript" => {
            let text = value.get("text")?.as_str()?.trim();
            if text.is_empty() {
                return None;
            }
            Some(SttTranscript::Partial(text.to_string()))
        }
        "committed_transcript" => {
            let text = value.get("text")?.as_str()?.trim();
            if text.is_empty() {
                return None;
            }
            Some(SttTranscript::Final(text.to_string()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::pipeline::SttTranscript;

    #[test]
    fn only_scribe_v2_realtime_allowed() {
        assert!(validate_stt_model("scribe_v2_realtime").is_ok());
        assert!(validate_stt_model("scribe_v2").is_err());
    }

    #[test]
    fn parse_partial_and_committed() {
        let partial = parse_realtime_message(
            r#"{"message_type":"partial_transcript","text":"hel"}"#,
        )
        .unwrap();
        assert_eq!(partial, SttTranscript::Partial("hel".into()));
        let fin = parse_realtime_message(
            r#"{"message_type":"committed_transcript","text":"hello"}"#,
        )
        .unwrap();
        assert_eq!(fin, SttTranscript::Final("hello".into()));
    }

    #[test]
    fn commit_chunk_shape() {
        let msg = input_audio_chunk_json("", true);
        assert!(msg.contains("\"commit\":true"));
        assert!(msg.contains("input_audio_chunk"));
    }
}
