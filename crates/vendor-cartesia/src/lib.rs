mod client;
mod factory;
mod matrix;
mod tts;

#[cfg(feature = "live")]
pub use client::{WsFailure, WsOutcome};
pub use factory::CartesiaFactory;
pub use matrix::{
    implemented_tts_transports, tts_delivery_plan, validate_model_id, websocket_url, BYTES_DOC,
    DEFAULT_API_VERSION, DEFAULT_BASE_URL, DEFAULT_MODEL_ID, DOCUMENTED_WS_MODELS,
    POOL_IDLE_TIMEOUT, TCP_KEEPALIVE, WS_DOC,
};
pub use tts::CartesiaTts;
