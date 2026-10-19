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
    listen_websocket_url_at(None, model, language)
}

/// Same as [`listen_websocket_url`] against `base` (`SttConfig.endpoint`, an `http(s)://` or
/// `ws(s)://` origin); `None` is the Deepgram API.
pub fn listen_websocket_url_at(
    base: Option<&str>,
    model: &str,
    language: Option<&str>,
) -> Result<String, String> {
    validate_listen_model(model)?;
    let base = base
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .unwrap_or(crate::tts_matrix::DEFAULT_BASE_URL)
        .trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    let mut url = format!(
        "{ws_base}/v1/listen?model={model}&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&punctuate=true"
    );
    if let Some(lang) = language.filter(|l| !l.is_empty()) {
        url.push_str("&language=");
        url.push_str(lang);
    }
    Ok(url)
}

/// Documented client message that flushes the stream: the server processes all unprocessed audio
/// and answers with `Results` that carry `from_finalize: true`.
pub const FINALIZE_MESSAGE: &str = r#"{"type":"Finalize"}"#;

/// A `Results` message reduced to what the provider acts on. Text is trimmed (may be empty).
#[derive(Debug, Clone, PartialEq)]
pub struct ListenResults {
    pub text: String,
    pub is_final: bool,
    /// `from_finalize`: the answer to a `Finalize` message.
    pub from_finalize: bool,
    /// `start` and `start + duration`, seconds of audio since the stream began.
    pub start_s: f64,
    pub end_s: f64,
}

/// Parse a documented `Results` WebSocket message (`type`, `is_final`, `from_finalize`, `start`,
/// `duration`, `channel.alternatives.0.transcript`). Other message types return `None`.
pub fn parse_listen_results(raw: &str) -> Option<ListenResults> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    if value.get("type")?.as_str()? != "Results" {
        return None;
    }
    let text = value
        .pointer("/channel/alternatives/0/transcript")?
        .as_str()?
        .trim()
        .to_string();
    let start_s = value.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let duration = value
        .get("duration")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    Some(ListenResults {
        text,
        is_final: value
            .get("is_final")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        from_finalize: value
            .get("from_finalize")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        start_s,
        end_s: start_s + duration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parse_interim_final_and_finalize_answer() {
        let r = parse_listen_results(
            r#"{"type":"Results","start":1.5,"duration":2.0,"is_final":false,"channel":{"alternatives":[{"transcript":"hello"}]}}"#,
        )
        .unwrap();
        assert!(!r.is_final && !r.from_finalize);
        assert_eq!((r.text.as_str(), r.start_s, r.end_s), ("hello", 1.5, 3.5));
        let r = parse_listen_results(
            r#"{"type":"Results","is_final":true,"from_finalize":true,"channel":{"alternatives":[{"transcript":""}]}}"#,
        )
        .unwrap();
        assert!(r.is_final && r.from_finalize && r.text.is_empty());
        assert!(parse_listen_results(r#"{"type":"Metadata"}"#).is_none());
    }

    #[test]
    fn listen_url_follows_the_endpoint_override() {
        let url = listen_websocket_url_at(Some("http://127.0.0.1:9/"), "nova-3", None).unwrap();
        assert!(url.starts_with("ws://127.0.0.1:9/v1/listen?model=nova-3&"));
    }
}
