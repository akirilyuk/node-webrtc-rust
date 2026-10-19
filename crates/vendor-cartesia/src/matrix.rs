//! Hardcoded Cartesia TTS capability matrix from published docs.
//!
//! REST bytes: <https://docs.cartesia.ai/api-reference/tts/bytes>
//! WebSocket: <https://docs.cartesia.ai/api-reference/tts/websocket>
//! Models enum on WS: sonic-3.6, sonic-3.5, sonic-3, sonic-latest

pub const WS_DOC: &str = "https://docs.cartesia.ai/api-reference/tts/websocket";
pub const BYTES_DOC: &str = "https://docs.cartesia.ai/api-reference/tts/bytes";

use std::sync::LazyLock;
use std::time::Duration;

use node_webrtc_rust_speech::config::SpeechTransport;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::resolve_transport;
use node_webrtc_rust_voice_catalog as voice_catalog;

/// TTS transports this crate implements. Lockstep with `voice_catalog::tts_transports("cartesia")`.
pub fn implemented_tts_transports() -> &'static [&'static str] {
    &["websocket", "http"]
}

/// Resolve `TtsConfig.transport` against the voice catalog (`None` / `auto` = catalog default).
pub fn resolve_tts_transport(requested: Option<SpeechTransport>) -> SpeechResult<SpeechTransport> {
    resolve_transport(
        "cartesia",
        "tts",
        requested,
        voice_catalog::tts_transports("cartesia"),
        voice_catalog::default_tts_transport("cartesia"),
    )?
    .ok_or_else(|| SpeechError::Config("cartesia tts has no catalog default transport".into()))
}

/// Default from current WebSocket `model_id` enum (not legacy `sonic-english`).
pub fn default_model_id() -> &'static str {
    voice_catalog::default_tts_model("cartesia").expect("cartesia default TTS in voice catalog")
}

/// Alias for callers that expect a `DEFAULT_MODEL_ID` name.
pub static DEFAULT_MODEL_ID: LazyLock<&'static str> = LazyLock::new(default_model_id);

pub static DOCUMENTED_WS_MODELS: LazyLock<&'static [&'static str]> =
    LazyLock::new(|| voice_catalog::tts_models("cartesia").expect("cartesia in voice catalog"));

/// Default REST base URL (`TtsConfig.endpoint` unset).
pub const DEFAULT_BASE_URL: &str = "https://api.cartesia.ai";

/// Documented `Cartesia-Version` / `cartesia_version` query (WS AsyncAPI).
pub const DEFAULT_API_VERSION: &str = "2026-08-14";

/// TCP + TLS + WebSocket handshake (also bounds the WS generation send and the HTTP connect).
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Wait for the first audio frame.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const FIRST_AUDIO_TIMEOUT: Duration = Duration::from_secs(15);
/// Wait between audio frames once audio has started.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
/// Whole REST request, headers through the last body byte.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long an idle pooled REST connection is kept for the next request.
pub const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// A kept TTS WebSocket is reused only if it finished an utterance within this window. The docs
/// give no idle timeout or maximum connection age for the TTS WebSocket, so this is our bound.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const WS_IDLE_REUSE: Duration = Duration::from_secs(60);
/// TCP keepalive on pooled REST connections.
pub const TCP_KEEPALIVE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsDeliveryPlan {
    WebSocketContexts,
    RestBytes,
}

pub fn validate_model_id(model: &str) -> Result<(), String> {
    if DOCUMENTED_WS_MODELS.contains(&model) {
        return Ok(());
    }
    Err(format!(
        "unsupported Cartesia model_id `{model}`; documented WebSocket models: {}",
        DOCUMENTED_WS_MODELS.join(", ")
    ))
}

pub fn tts_delivery_plan(_model: &str) -> TtsDeliveryPlan {
    TtsDeliveryPlan::WebSocketContexts
}

/// WS URL for a REST base URL: `https://` → `wss://`, `http://` → `ws://`.
pub fn websocket_url(base_url: &str, api_version: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{ws_base}/tts/websocket?cartesia_version={api_version}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implemented_transports_match_the_catalog() {
        let mut implemented = implemented_tts_transports().to_vec();
        let mut catalog = voice_catalog::tts_transports("cartesia").to_vec();
        implemented.sort_unstable();
        catalog.sort_unstable();
        assert_eq!(implemented, catalog);
    }

    #[test]
    fn auto_is_websocket_and_http_is_selectable() {
        assert_eq!(
            resolve_tts_transport(None).unwrap(),
            SpeechTransport::Websocket
        );
        assert_eq!(
            resolve_tts_transport(Some(SpeechTransport::Auto)).unwrap(),
            SpeechTransport::Websocket
        );
        assert_eq!(
            resolve_tts_transport(Some(SpeechTransport::Http)).unwrap(),
            SpeechTransport::Http
        );
    }

    #[test]
    fn default_model_in_documented_set() {
        assert!(DOCUMENTED_WS_MODELS.contains(&*DEFAULT_MODEL_ID));
    }

    #[test]
    fn sonic_english_not_in_ws_enum() {
        assert!(validate_model_id("sonic-english").is_err());
    }

    #[test]
    fn websocket_url_includes_version() {
        assert!(websocket_url(DEFAULT_BASE_URL, DEFAULT_API_VERSION).contains("cartesia_version="));
    }

    #[test]
    fn websocket_url_default_unchanged() {
        assert_eq!(
            websocket_url(DEFAULT_BASE_URL, DEFAULT_API_VERSION),
            "wss://api.cartesia.ai/tts/websocket?cartesia_version=2026-08-14"
        );
    }

    #[test]
    fn websocket_url_from_http_endpoint() {
        assert_eq!(
            websocket_url("http://127.0.0.1:9/", "v"),
            "ws://127.0.0.1:9/tts/websocket?cartesia_version=v"
        );
    }
}
