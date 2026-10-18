//! OpenAI STT/TTS vendor adapter (live API behind `live` feature).

mod factory;
#[cfg(feature = "live")]
mod http;
mod matrix;
mod stt;
mod tts;

pub use matrix::{
    documented_realtime_model, stt_default_transport, stt_file_sse_supported, SttDefaultTransport,
    SttRealtimeMode, CONNECT_TIMEOUT, DOCUMENTED_STT_MODELS, HTTP_TOTAL_TIMEOUT, POOL_IDLE_TIMEOUT,
    TCP_KEEPALIVE,
};

pub use factory::OpenAiFactory;
pub use stt::OpenAiStt;
pub use tts::OpenAiTts;
