//! Hardcoded Fish Audio TTS capability matrix from published docs.
//!
//! WebSocket: <https://docs.fish.audio/api-reference/endpoint/websocket/tts-live>
//! HTTP: <https://docs.fish.audio/api-reference/endpoint/openapi-v1/text-to-speech>
//! Drama 3: <https://docs.fish.audio/features/text-to-speech/drama-3>
//! Realtime streaming: <https://docs.fish.audio/features/realtime-streaming>

use std::sync::LazyLock;
use std::time::Duration;

use node_webrtc_rust_speech::config::SpeechTransport;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::resolve_transport;
use node_webrtc_rust_voice_catalog as voice_catalog;

/// TTS transports this crate implements. Lockstep with `voice_catalog::tts_transports("fishaudio")`.
pub fn implemented_tts_transports() -> &'static [&'static str] {
    &["http", "websocket"]
}

/// Resolve `TtsConfig.transport` against the voice catalog (`None` / `auto` = catalog default).
///
/// `http` (the default) keeps the documented WebSocket fallback while no audio has reached the
/// sink; an explicit `websocket` uses the socket only, without a fallback.
pub fn resolve_tts_transport(requested: Option<SpeechTransport>) -> SpeechResult<SpeechTransport> {
    resolve_transport(
        "fishaudio",
        "tts",
        requested,
        voice_catalog::tts_transports("fishaudio"),
        voice_catalog::default_tts_transport("fishaudio"),
    )?
    .ok_or_else(|| SpeechError::Config("fishaudio tts has no catalog default transport".into()))
}

pub const WS_DOC: &str = "https://docs.fish.audio/api-reference/endpoint/websocket/tts-live";
pub const HTTP_DOC: &str =
    "https://docs.fish.audio/api-reference/endpoint/openapi-v1/text-to-speech";
pub const DRAMA3_DOC: &str = "https://docs.fish.audio/features/text-to-speech/drama-3";
pub const REALTIME_DOC: &str = "https://docs.fish.audio/features/realtime-streaming";

/// Default API origin. `TtsConfig.endpoint` replaces it (tests point it at a local mock).
pub const DEFAULT_BASE_URL: &str = "https://api.fish.audio";

/// Requested PCM sample rate. 48 kHz is not documented for PCM, so we request 24 kHz and
/// upsample x2 locally.
pub const PCM_SAMPLE_RATE: u32 = 24_000;

/// Documented `latency` value for the lowest time to first audio (voice agents).
pub const LATENCY: &str = "balanced";

/// TCP + TLS + WebSocket handshake (also bounds each WS send and the HTTP connect).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Wait for the first audio frame / first HTTP body chunk.
pub const FIRST_AUDIO_TIMEOUT: Duration = Duration::from_secs(15);
/// Wait between audio frames once audio has started.
pub const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
/// Whole HTTP request, headers through the last body chunk.
pub const HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long an idle pooled HTTP connection is kept for the next request.
pub const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// TCP keepalive on pooled HTTP connections.
pub const TCP_KEEPALIVE: Duration = Duration::from_secs(30);

/// Default model: Drama 3.
pub fn default_model_id() -> &'static str {
    voice_catalog::default_tts_model("fishaudio").expect("fishaudio default TTS in voice catalog")
}

/// Alias for callers that expect a `DEFAULT_MODEL_ID` name.
pub static DEFAULT_MODEL_ID: LazyLock<&'static str> = LazyLock::new(default_model_id);

pub static DOCUMENTED_MODELS: LazyLock<&'static [&'static str]> =
    LazyLock::new(|| voice_catalog::tts_models("fishaudio").expect("fishaudio in voice catalog"));

/// Fish silently falls back to `s2.1-pro` for a missing or misspelled `model` header, so an
/// unknown id is rejected before anything is sent.
pub fn validate_model_id(model: &str) -> Result<(), String> {
    if DOCUMENTED_MODELS.contains(&model) {
        return Ok(());
    }
    Err(format!(
        "unsupported Fish Audio model `{model}`; documented models: {}",
        DOCUMENTED_MODELS.join(", ")
    ))
}

fn trim_base(base: &str) -> &str {
    base.trim_end_matches('/')
}

/// `POST {base}/v1/tts`
pub fn http_tts_url(base: &str) -> String {
    format!("{}/v1/tts", trim_base(base))
}

/// `{base}/v1/tts/live` with `https` mapped to `wss` and `http` to `ws`.
pub fn ws_live_url(base: &str) -> String {
    let base = trim_base(base);
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{ws_base}/v1/tts/live")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implemented_transports_match_the_catalog() {
        let mut implemented = implemented_tts_transports().to_vec();
        let mut catalog = voice_catalog::tts_transports("fishaudio").to_vec();
        implemented.sort_unstable();
        catalog.sort_unstable();
        assert_eq!(implemented, catalog);
    }

    #[test]
    fn auto_is_http_and_websocket_is_selectable() {
        assert_eq!(resolve_tts_transport(None).unwrap(), SpeechTransport::Http);
        assert_eq!(
            resolve_tts_transport(Some(SpeechTransport::Auto)).unwrap(),
            SpeechTransport::Http
        );
        assert_eq!(
            resolve_tts_transport(Some(SpeechTransport::Websocket)).unwrap(),
            SpeechTransport::Websocket
        );
    }

    #[test]
    fn default_model_is_drama_3() {
        assert_eq!(*DEFAULT_MODEL_ID, "drama-3");
        assert!(DOCUMENTED_MODELS.contains(&*DEFAULT_MODEL_ID));
    }

    #[test]
    fn s1_and_drama3_are_rejected() {
        assert!(validate_model_id("s1").is_err());
        assert!(validate_model_id("drama3").is_err());
        assert!(validate_model_id("").is_err());
        for model in ["drama-3", "s2.1-pro", "s2.1-pro-free", "s2-pro"] {
            assert!(validate_model_id(model).is_ok(), "{model}");
        }
    }

    #[test]
    fn error_lists_allowed_ids() {
        let err = validate_model_id("nope").unwrap_err();
        assert!(err.contains("drama-3") && err.contains("s2-pro"), "{err}");
    }

    #[test]
    fn url_mapping_for_https_base() {
        assert_eq!(
            ws_live_url("https://api.fish.audio"),
            "wss://api.fish.audio/v1/tts/live"
        );
        assert_eq!(
            http_tts_url("https://api.fish.audio/"),
            "https://api.fish.audio/v1/tts"
        );
    }

    #[test]
    fn url_mapping_for_http_base() {
        assert_eq!(
            ws_live_url("http://127.0.0.1:8080"),
            "ws://127.0.0.1:8080/v1/tts/live"
        );
        assert_eq!(
            http_tts_url("http://127.0.0.1:8080"),
            "http://127.0.0.1:8080/v1/tts"
        );
    }
}
