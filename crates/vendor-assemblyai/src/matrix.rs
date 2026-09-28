//! Hardcoded AssemblyAI streaming STT matrix from published docs.
//!
//! Streaming quickstart: <https://www.assemblyai.com/docs/speech-to-text/streaming>
//! WebSocket: `wss://streaming.assemblyai.com/v3/ws` with `speech_model` query.

pub const STREAMING_DOC: &str = "https://www.assemblyai.com/docs/speech-to-text/streaming";

use std::sync::LazyLock;

use node_webrtc_rust_voice_catalog as voice_catalog;

/// Documented `speech_model` values from the streaming quickstart / WebSocket examples.
pub static DOCUMENTED_SPEECH_MODELS: LazyLock<&'static [&'static str]> =
    LazyLock::new(|| voice_catalog::stt_models("assemblyai").expect("assemblyai in voice catalog"));

pub const STREAMING_WS_BASE: &str = "wss://streaming.assemblyai.com/v3/ws";

pub fn default_speech_model() -> &'static str {
    voice_catalog::default_stt_model("assemblyai").expect("assemblyai default STT in voice catalog")
}

pub fn validate_speech_model(model: &str) -> Result<(), String> {
    if DOCUMENTED_SPEECH_MODELS.contains(&model) {
        return Ok(());
    }
    Err(format!(
        "unsupported AssemblyAI speech_model `{model}`; documented models: {}",
        DOCUMENTED_SPEECH_MODELS.join(", ")
    ))
}

/// PCM mono 16 kHz — documented encoding for VoiceAgent pipeline.
pub fn streaming_websocket_url(speech_model: &str) -> Result<String, String> {
    validate_speech_model(speech_model)?;
    Ok(format!(
        "{STREAMING_WS_BASE}?speech_model={speech_model}&encoding=pcm_s16le&sample_rate=16000"
    ))
}

/// Parse documented v3 `Turn` message (`type`, `transcript`, `end_of_turn`).
pub fn parse_turn_message(raw: &str) -> Option<node_webrtc_rust_speech::pipeline::SttTranscript> {
    use node_webrtc_rust_speech::pipeline::SttTranscript;
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    if value.get("type")?.as_str()? != "Turn" {
        return None;
    }
    let text = value.get("transcript")?.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    let end_of_turn = value
        .get("end_of_turn")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if end_of_turn {
        Some(SttTranscript::Final(text.to_string()))
    } else {
        Some(SttTranscript::Partial(text.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::pipeline::SttTranscript;

    #[test]
    fn documented_models_build_url() {
        for model in *DOCUMENTED_SPEECH_MODELS {
            let url = streaming_websocket_url(model).unwrap();
            assert!(url.contains("speech_model="));
            assert!(url.contains("pcm_s16le"));
        }
    }

    #[test]
    fn parse_turn_partial_and_final() {
        let partial =
            parse_turn_message(r#"{"type":"Turn","transcript":"hello","end_of_turn":false}"#)
                .unwrap();
        assert_eq!(partial, SttTranscript::Partial("hello".into()));
        let fin =
            parse_turn_message(r#"{"type":"Turn","transcript":"hello world","end_of_turn":true}"#)
                .unwrap();
        assert_eq!(fin, SttTranscript::Final("hello world".into()));
    }
}
