#[cfg(feature = "live")]
mod auth;
mod factory;
#[cfg(feature = "live")]
mod http;
mod matrix;
mod stt;
#[cfg(feature = "live")]
mod stt_live;
mod tts;
#[cfg(feature = "live")]
mod tts_live;

pub use factory::GoogleFactory;
pub use matrix::{
    default_stt_model, stt_default_transport, voice_supports_streaming_synthesize,
    GoogleSttV2Locator, SttDefaultTransport, CONNECT_TIMEOUT, DOCUMENTED_STT_MODELS,
    HTTP_TOTAL_TIMEOUT, POOL_IDLE_TIMEOUT, STT_V2_RECOGNIZER_DOC, STT_V2_STREAMING_DOC,
    TCP_KEEPALIVE, TTS_STREAMING_DOC,
};
pub use stt::GoogleStt;
pub use tts::GoogleTts;
