mod factory;
mod matrix;
mod stt;
mod tts;
mod tts_matrix;

pub use factory::DeepgramFactory;
pub use matrix::{
    default_listen_model, listen_websocket_url, validate_listen_model, DOCUMENTED_LISTEN_MODELS,
    LISTEN_DOC,
};
pub use tts_matrix::{
    default_speak_model, validate_speak_model, DEFAULT_AURA_MODEL, DEFAULT_FLUX_MODEL,
    V1_SPEAK_DOC, V2_SPEAK_DOC,
};
