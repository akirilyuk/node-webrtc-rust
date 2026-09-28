mod client;
mod factory;
mod matrix;
mod stt;

pub use factory::AssemblyAiFactory;
pub use matrix::{
    default_speech_model, streaming_websocket_url, validate_speech_model,
    DOCUMENTED_SPEECH_MODELS, STREAMING_DOC, STREAMING_WS_BASE,
};
