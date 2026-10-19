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
    streaming_websocket_url_for(speech_model, None)
}

/// Like [`streaming_websocket_url`] with `SttConfig.endpoint` as the host: `https://` maps to
/// `wss://`, `http://` to `ws://` (the docs list regional streaming hosts; the local mock-server
/// tests use `http://127.0.0.1:<port>`). The path stays `/v3/ws`.
pub fn streaming_websocket_url_for(
    speech_model: &str,
    endpoint: Option<&str>,
) -> Result<String, String> {
    validate_speech_model(speech_model)?;
    let base = match endpoint.map(|e| e.trim().trim_end_matches('/')) {
        Some(e) if !e.is_empty() => {
            if let Some(rest) = e.strip_prefix("https://") {
                format!("wss://{rest}/v3/ws")
            } else if let Some(rest) = e.strip_prefix("http://") {
                format!("ws://{rest}/v3/ws")
            } else {
                format!("{e}/v3/ws")
            }
        }
        _ => STREAMING_WS_BASE.to_string(),
    };
    Ok(format!(
        "{base}?speech_model={speech_model}&encoding=pcm_s16le&sample_rate=16000"
    ))
}

/// Bound for the WebSocket connect and for every send on the socket
/// (`infra.unbounded-external-wait`).
pub const WS_BOUND: std::time::Duration = std::time::Duration::from_secs(10);
/// How long `disconnect` waits for the socket task to send `Terminate` before aborting it.
pub const DISCONNECT_BOUND: std::time::Duration = std::time::Duration::from_secs(3);

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
    fn endpoint_replaces_the_host_and_keeps_the_path() {
        assert_eq!(
            streaming_websocket_url_for("universal-streaming-english", None).unwrap(),
            streaming_websocket_url("universal-streaming-english").unwrap()
        );
        let url =
            streaming_websocket_url_for("universal-streaming-english", Some("http://127.0.0.1:9/"))
                .unwrap();
        assert!(
            url.starts_with("ws://127.0.0.1:9/v3/ws?speech_model="),
            "{url}"
        );
        let url = streaming_websocket_url_for(
            "universal-streaming-english",
            Some("https://streaming.eu.assemblyai.com"),
        )
        .unwrap();
        assert!(
            url.starts_with("wss://streaming.eu.assemblyai.com/v3/ws?"),
            "{url}"
        );
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

/// Validate `SttConfig.transport` against the voice catalog (`None` / `auto` = the
/// catalog default). An unsupported value is a config error, raised at provider construction.
pub(crate) fn check_stt_transport(
    requested: Option<node_webrtc_rust_speech::config::SpeechTransport>,
) -> node_webrtc_rust_speech::error::SpeechResult<()> {
    node_webrtc_rust_speech::resolve_transport(
        "assemblyai",
        "stt",
        requested,
        voice_catalog::stt_transports("assemblyai"),
        voice_catalog::default_stt_transport("assemblyai"),
    )
    .map(|_| ())
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use node_webrtc_rust_speech::config::SpeechTransport;
    use node_webrtc_rust_speech::error::SpeechError;

    #[test]
    fn stt_transport_accepts_auto_and_websocket_and_rejects_http() {
        assert_eq!(voice_catalog::stt_transports("assemblyai"), &["websocket"]);
        assert!(check_stt_transport(None).is_ok());
        assert!(check_stt_transport(Some(SpeechTransport::Auto)).is_ok());
        assert!(check_stt_transport(Some(SpeechTransport::Websocket)).is_ok());
        match check_stt_transport(Some(SpeechTransport::Http)) {
            Err(SpeechError::Config(message)) => assert_eq!(
                message,
                "assemblyai stt transport 'http' is not supported; allowed: auto, websocket"
            ),
            other => panic!("expected a Config error, got {other:?}"),
        }
    }
}
