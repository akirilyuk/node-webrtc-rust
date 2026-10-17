//! Hardcoded ElevenLabs TTS capability matrix from published docs.
//!
//! HTTP streaming: <https://elevenlabs.io/docs/api-reference/text-to-speech/stream>
//! WebSocket stream-input: <https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/realtime-tts>
//! (`/v1/text-to-speech/{voice_id}/stream-input` does not support `eleven_v3`.)

use std::time::Duration;

pub const WS_STREAM_INPUT_DOC: &str =
    "https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/realtime-tts";
pub const HTTP_STREAM_DOC: &str = "https://elevenlabs.io/docs/api-reference/text-to-speech/stream";

pub const DEFAULT_MODEL_ID: &str = "eleven_multilingual_v2";

/// Default REST base URL (`TtsConfig.endpoint` unset). The WebSocket host is derived from it.
pub const DEFAULT_BASE_URL: &str = "https://api.elevenlabs.io";

/// TCP + TLS connect of every HTTP request.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Wait for the response headers and the first audio body chunk.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const FIRST_AUDIO_TIMEOUT: Duration = Duration::from_secs(15);
/// Wait between audio body chunks once audio has started.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
/// Whole full-body HTTP request, headers through the last body byte.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(60);

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

pub fn tts_progressive_transport(model: &str) -> TtsProgressiveTransport {
    if model == "eleven_v3" {
        TtsProgressiveTransport::FullBodyPost
    } else {
        TtsProgressiveTransport::HttpStream
    }
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
