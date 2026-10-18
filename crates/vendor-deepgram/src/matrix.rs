//! Hardcoded Deepgram live listen model matrix from published docs.
//!
//! Live listen: <https://developers.deepgram.com/docs/live-streaming-audio>
//! Models: <https://developers.deepgram.com/docs/models>

use std::sync::LazyLock;

use node_webrtc_rust_speech::config::SpeechTransport;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::resolve_transport;
use node_webrtc_rust_voice_catalog as voice_catalog;

/// STT transports this crate implements. Lockstep with `voice_catalog::stt_transports("deepgram")`.
pub fn implemented_stt_transports() -> &'static [&'static str] {
    &["websocket"]
}

/// Resolve `SttConfig.transport` against the voice catalog (`None` / `auto` = catalog default).
pub fn resolve_stt_transport(requested: Option<SpeechTransport>) -> SpeechResult<SpeechTransport> {
    resolve_transport(
        "deepgram",
        "stt",
        requested,
        voice_catalog::stt_transports("deepgram"),
        voice_catalog::default_stt_transport("deepgram"),
    )?
    .ok_or_else(|| SpeechError::Config("deepgram stt has no catalog default transport".into()))
}

/// Documented `model` query values for `wss://api.deepgram.com/v1/listen`.
pub static DOCUMENTED_LISTEN_MODELS: LazyLock<&'static [&'static str]> =
    LazyLock::new(|| voice_catalog::stt_models("deepgram").expect("deepgram in voice catalog"));

pub const LISTEN_DOC: &str = "https://developers.deepgram.com/docs/live-streaming-audio";

pub fn default_listen_model() -> &'static str {
    voice_catalog::default_stt_model("deepgram").expect("deepgram default STT in voice catalog")
}

pub fn validate_listen_model(model: &str) -> Result<(), String> {
    if DOCUMENTED_LISTEN_MODELS.contains(&model) {
        return Ok(());
    }
    Err(format!(
        "unsupported Deepgram listen model `{model}`; documented models: {}",
        DOCUMENTED_LISTEN_MODELS.join(", ")
    ))
}

/// Build documented live listen WebSocket URL (query params from live streaming guide).
pub fn listen_websocket_url(model: &str, language: Option<&str>) -> Result<String, String> {
    validate_listen_model(model)?;
    let mut url = format!(
        "wss://api.deepgram.com/v1/listen?model={model}&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&punctuate=true"
    );
    if let Some(lang) = language.filter(|l| !l.is_empty()) {
        url.push_str("&language=");
        url.push_str(lang);
    }
    Ok(url)
}

/// Parse documented `Results` WebSocket message (`type`, `is_final`, `channel.alternatives.0.transcript`).
pub fn parse_listen_results_message(
    raw: &str,
) -> Option<node_webrtc_rust_speech::pipeline::SttTranscript> {
    use node_webrtc_rust_speech::pipeline::SttTranscript;
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    if value.get("type")?.as_str()? != "Results" {
        return None;
    }
    let transcript = value
        .pointer("/channel/alternatives/0/transcript")?
        .as_str()?
        .trim();
    if transcript.is_empty() {
        return None;
    }
    let is_final = value
        .get("is_final")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if is_final {
        Some(SttTranscript::Final(transcript.to_string()))
    } else {
        Some(SttTranscript::Partial(transcript.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::pipeline::SttTranscript;

    #[test]
    fn implemented_transports_match_the_catalog() {
        let mut implemented = implemented_stt_transports().to_vec();
        let mut catalog = voice_catalog::stt_transports("deepgram").to_vec();
        implemented.sort_unstable();
        catalog.sort_unstable();
        assert_eq!(implemented, catalog);
    }

    #[test]
    fn stt_transport_auto_is_websocket_and_http_is_rejected() {
        assert_eq!(
            resolve_stt_transport(None).unwrap(),
            SpeechTransport::Websocket
        );
        assert_eq!(
            resolve_stt_transport(Some(SpeechTransport::Auto)).unwrap(),
            SpeechTransport::Websocket
        );
        match resolve_stt_transport(Some(SpeechTransport::Http)) {
            Err(SpeechError::Config(message)) => assert_eq!(
                message,
                "deepgram stt transport 'http' is not supported; allowed: auto, websocket"
            ),
            other => panic!("expected a Config error, got {other:?}"),
        }
    }

    #[test]
    fn nova_models_allowed() {
        assert!(validate_listen_model("nova-2").is_ok());
        assert!(validate_listen_model("nova-3").is_ok());
    }

    #[test]
    fn unknown_model_errors() {
        assert!(validate_listen_model("flux").is_err());
    }

    #[test]
    fn listen_url_includes_interim_results() {
        let url = listen_websocket_url("nova-3", Some("en")).unwrap();
        assert!(url.contains("interim_results=true"));
        assert!(url.contains("model=nova-3"));
    }

    #[test]
    fn parse_interim_and_final() {
        let partial = parse_listen_results_message(
            r#"{"type":"Results","is_final":false,"channel":{"alternatives":[{"transcript":"hello"}]}}"#,
        )
        .unwrap();
        assert_eq!(partial, SttTranscript::Partial("hello".into()));
        let fin = parse_listen_results_message(
            r#"{"type":"Results","is_final":true,"channel":{"alternatives":[{"transcript":"hello world"}]}}"#,
        )
        .unwrap();
        assert_eq!(fin, SttTranscript::Final("hello world".into()));
    }
}
