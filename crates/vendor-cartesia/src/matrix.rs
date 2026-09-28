//! Hardcoded Cartesia TTS capability matrix from published docs.
//!
//! REST bytes: <https://docs.cartesia.ai/api-reference/tts/bytes>
//! WebSocket: <https://docs.cartesia.ai/api-reference/tts/websocket>
//! Models enum on WS: sonic-3.6, sonic-3.5, sonic-3, sonic-latest

pub const WS_DOC: &str = "https://docs.cartesia.ai/api-reference/tts/websocket";
pub const BYTES_DOC: &str = "https://docs.cartesia.ai/api-reference/tts/bytes";

/// Default from current WebSocket `model_id` enum (not legacy `sonic-english`).
pub const DEFAULT_MODEL_ID: &str = "sonic-3";

pub const DOCUMENTED_WS_MODELS: &[&str] = &["sonic-3.6", "sonic-3.5", "sonic-3", "sonic-latest"];

/// Documented `Cartesia-Version` / `cartesia_version` query (WS AsyncAPI).
pub const DEFAULT_API_VERSION: &str = "2026-08-14";

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

pub fn websocket_url(api_version: &str) -> String {
    format!("wss://api.cartesia.ai/tts/websocket?cartesia_version={api_version}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_in_documented_set() {
        assert!(DOCUMENTED_WS_MODELS.contains(&DEFAULT_MODEL_ID));
    }

    #[test]
    fn sonic_english_not_in_ws_enum() {
        assert!(validate_model_id("sonic-english").is_err());
    }

    #[test]
    fn websocket_url_includes_version() {
        assert!(websocket_url(DEFAULT_API_VERSION).contains("cartesia_version="));
    }
}
