//! Hardcoded ElevenLabs TTS capability matrix from published docs.
//!
//! HTTP streaming: <https://elevenlabs.io/docs/api-reference/text-to-speech/stream>
//! WebSocket stream-input: <https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/realtime-tts>
//! (`/v1/text-to-speech/{voice_id}/stream-input` does not support `eleven_v3`.)

use std::time::Duration;

use node_webrtc_rust_speech::config::SpeechTransport;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::resolve_transport;
use node_webrtc_rust_voice_catalog as voice_catalog;

pub const WS_STREAM_INPUT_DOC: &str =
    "https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/realtime-tts";
pub const HTTP_STREAM_DOC: &str = "https://elevenlabs.io/docs/api-reference/text-to-speech/stream";

pub const DEFAULT_MODEL_ID: &str = "eleven_multilingual_v2";

/// Default REST base URL (`TtsConfig.endpoint` unset). The WebSocket host is derived from it.
pub const DEFAULT_BASE_URL: &str = "https://api.elevenlabs.io";

/// TCP + TLS connect of every HTTP request.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// TCP + TLS + WebSocket upgrade handshake, and each WebSocket control send
/// (`infra.unbounded-external-wait`).
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Wait for the response headers and the first audio body chunk.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const FIRST_AUDIO_TIMEOUT: Duration = Duration::from_secs(15);
/// Wait between audio body chunks once audio has started.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
/// Whole full-body HTTP request, headers through the last body byte.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long an idle pooled HTTP connection is kept for the next request.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// TCP keepalive on pooled HTTP connections.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const TCP_KEEPALIVE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsDeliveryPlan {
    /// Chunked HTTP `POST …/text-to-speech/{voice_id}/stream`.
    HttpStream,
    /// Full-body `POST …/text-to-speech/{voice_id}` only.
    FullBodyPost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsProgressiveTransport {
    HttpStream,
    WebSocketStreamInput,
    FullBodyPost,
}

pub fn tts_delivery_plan(model: &str) -> TtsDeliveryPlan {
    if model == "eleven_v3" {
        TtsDeliveryPlan::FullBodyPost
    } else {
        TtsDeliveryPlan::HttpStream
    }
}

pub fn tts_websocket_allowed(model: &str) -> bool {
    model != "eleven_v3"
}

/// Progressive transport for the `http` customer transport (the catalog default).
pub fn tts_progressive_transport(model: &str) -> TtsProgressiveTransport {
    tts_progressive_transport_for(model, SpeechTransport::Http)
}

/// Progressive transport for a resolved customer transport. `websocket` is never offered for
/// `eleven_v3` (see [`tts_websocket_allowed`]); [`resolve_tts_transport`] rejects that pair.
pub fn tts_progressive_transport_for(
    model: &str,
    transport: SpeechTransport,
) -> TtsProgressiveTransport {
    if transport == SpeechTransport::Websocket && tts_websocket_allowed(model) {
        return TtsProgressiveTransport::WebSocketStreamInput;
    }
    if model == "eleven_v3" {
        TtsProgressiveTransport::FullBodyPost
    } else {
        TtsProgressiveTransport::HttpStream
    }
}

/// TTS transports this crate implements. Lockstep with `voice_catalog::tts_transports("elevenlabs")`.
pub fn implemented_tts_transports() -> &'static [&'static str] {
    &["http", "websocket"]
}

/// Resolve `TtsConfig.transport` for `model` against the voice catalog (`None` / `auto` = default).
/// `websocket` with a model the stream-input socket does not support (`eleven_v3`) is a config error.
pub fn resolve_tts_transport(
    model: &str,
    requested: Option<SpeechTransport>,
) -> SpeechResult<SpeechTransport> {
    let resolved = resolve_transport(
        "elevenlabs",
        "tts",
        requested,
        voice_catalog::tts_transports("elevenlabs"),
        voice_catalog::default_tts_transport("elevenlabs"),
    )?
    .ok_or_else(|| SpeechError::Config("elevenlabs tts has no catalog default transport".into()))?;
    if resolved == SpeechTransport::Websocket && !tts_websocket_allowed(model) {
        return Err(SpeechError::Config(format!(
            "elevenlabs tts transport 'websocket' is not supported for model '{model}'; use http or auto"
        )));
    }
    Ok(resolved)
}

pub fn http_stream_url(base_url: &str, voice_id: &str) -> String {
    format!(
        "{}/v1/text-to-speech/{voice_id}/stream?output_format=pcm_48000",
        base_url.trim_end_matches('/')
    )
}

/// Full-body `POST …/text-to-speech/{voice_id}`.
pub fn full_body_url(base_url: &str, voice_id: &str) -> String {
    format!(
        "{}/v1/text-to-speech/{voice_id}?output_format=pcm_48000",
        base_url.trim_end_matches('/')
    )
}

pub fn websocket_stream_input_url(base_url: &str, voice_id: &str, model_id: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{ws_base}/v1/text-to-speech/{voice_id}/stream-input?model_id={model_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eleven_v3_no_websocket() {
        assert!(!tts_websocket_allowed("eleven_v3"));
        assert_eq!(
            tts_delivery_plan("eleven_v3"),
            TtsDeliveryPlan::FullBodyPost
        );
    }

    #[test]
    fn implemented_transports_match_the_catalog() {
        let mut implemented = implemented_tts_transports().to_vec();
        let mut catalog = voice_catalog::tts_transports("elevenlabs").to_vec();
        implemented.sort_unstable();
        catalog.sort_unstable();
        assert_eq!(implemented, catalog);
    }

    #[test]
    fn auto_is_http_and_websocket_is_selectable() {
        let m = "eleven_flash_v2_5";
        assert_eq!(
            resolve_tts_transport(m, None).unwrap(),
            SpeechTransport::Http
        );
        assert_eq!(
            resolve_tts_transport(m, Some(SpeechTransport::Auto)).unwrap(),
            SpeechTransport::Http
        );
        assert_eq!(
            resolve_tts_transport(m, Some(SpeechTransport::Websocket)).unwrap(),
            SpeechTransport::Websocket
        );
    }

    #[test]
    fn websocket_is_rejected_for_eleven_v3() {
        match resolve_tts_transport("eleven_v3", Some(SpeechTransport::Websocket)) {
            Err(SpeechError::Config(message)) => {
                assert!(message.contains("eleven_v3"), "{message}")
            }
            other => panic!("expected a Config error, got {other:?}"),
        }
        assert_eq!(
            resolve_tts_transport("eleven_v3", None).unwrap(),
            SpeechTransport::Http
        );
    }

    #[test]
    fn progressive_transport_follows_the_resolved_transport() {
        assert_eq!(
            tts_progressive_transport_for("eleven_flash_v2_5", SpeechTransport::Websocket),
            TtsProgressiveTransport::WebSocketStreamInput
        );
        assert_eq!(
            tts_progressive_transport_for("eleven_flash_v2_5", SpeechTransport::Http),
            TtsProgressiveTransport::HttpStream
        );
        assert_eq!(
            tts_progressive_transport_for("eleven_v3", SpeechTransport::Http),
            TtsProgressiveTransport::FullBodyPost
        );
    }

    #[test]
    fn flash_uses_http_stream() {
        assert_eq!(
            tts_delivery_plan("eleven_flash_v2_5"),
            TtsDeliveryPlan::HttpStream
        );
        assert!(tts_websocket_allowed("eleven_flash_v2_5"));
    }

    #[test]
    fn default_urls_unchanged() {
        assert_eq!(
            http_stream_url(DEFAULT_BASE_URL, "v1"),
            "https://api.elevenlabs.io/v1/text-to-speech/v1/stream?output_format=pcm_48000"
        );
        assert_eq!(
            full_body_url(DEFAULT_BASE_URL, "v1"),
            "https://api.elevenlabs.io/v1/text-to-speech/v1?output_format=pcm_48000"
        );
        assert_eq!(
            websocket_stream_input_url(DEFAULT_BASE_URL, "v1", "m"),
            "wss://api.elevenlabs.io/v1/text-to-speech/v1/stream-input?model_id=m"
        );
    }

    #[test]
    fn http_endpoint_maps_to_ws() {
        assert_eq!(
            websocket_stream_input_url("http://127.0.0.1:9/", "v1", "m"),
            "ws://127.0.0.1:9/v1/text-to-speech/v1/stream-input?model_id=m"
        );
        assert_eq!(
            http_stream_url("http://127.0.0.1:9/", "v1"),
            "http://127.0.0.1:9/v1/text-to-speech/v1/stream?output_format=pcm_48000"
        );
        assert_eq!(
            full_body_url("http://127.0.0.1:9", "v1"),
            "http://127.0.0.1:9/v1/text-to-speech/v1?output_format=pcm_48000"
        );
    }
}
