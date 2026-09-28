//! Hardcoded ElevenLabs TTS capability matrix from published docs.
//!
//! HTTP streaming: <https://elevenlabs.io/docs/api-reference/text-to-speech/stream>
//! WebSocket stream-input: <https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/realtime-tts>
//! (`/v1/text-to-speech/{voice_id}/stream-input` does not support `eleven_v3`.)

pub const WS_STREAM_INPUT_DOC: &str =
    "https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/realtime-tts";
pub const HTTP_STREAM_DOC: &str =
    "https://elevenlabs.io/docs/api-reference/text-to-speech/stream";

pub const DEFAULT_MODEL_ID: &str = "eleven_multilingual_v2";

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

pub fn http_stream_url(voice_id: &str) -> String {
    format!(
        "https://api.elevenlabs.io/v1/text-to-speech/{voice_id}/stream?output_format=pcm_48000"
    )
}

pub fn websocket_stream_input_url(voice_id: &str, model_id: &str) -> String {
    format!(
        "wss://api.elevenlabs.io/v1/text-to-speech/{voice_id}/stream-input?model_id={model_id}"
    )
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
}
