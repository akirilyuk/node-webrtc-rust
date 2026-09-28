//! OpenAI STT/TTS vendor adapter (live API behind `live` feature).

mod factory;
mod matrix;
mod stt;
mod tts;

pub use matrix::{
    documented_realtime_model, stt_default_transport, stt_file_sse_supported,
    DOCUMENTED_STT_MODELS, SttDefaultTransport, SttRealtimeMode,
};

pub use factory::OpenAiFactory;
pub use stt::OpenAiStt;
pub use tts::OpenAiTts;
