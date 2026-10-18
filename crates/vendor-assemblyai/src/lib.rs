mod client;
mod factory;
mod matrix;
mod stt;
mod ws_connect;

pub use factory::AssemblyAiFactory;
pub use matrix::{
    default_speech_model, streaming_websocket_url, streaming_websocket_url_for,
    validate_speech_model, DOCUMENTED_SPEECH_MODELS, STREAMING_DOC, STREAMING_WS_BASE,
};
