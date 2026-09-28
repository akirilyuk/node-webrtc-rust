#[cfg(feature = "live")]
mod auth;
#[cfg(feature = "live")]
mod stt_live;
#[cfg(feature = "live")]
mod tts_live;
mod factory;
mod matrix;
mod stt;
mod tts;

pub use factory::GoogleFactory;
pub use matrix::{
    default_stt_model, stt_default_transport, voice_supports_streaming_synthesize,
    DOCUMENTED_STT_MODELS, GoogleSttV2Locator, SttDefaultTransport, STT_V2_RECOGNIZER_DOC,
    STT_V2_STREAMING_DOC, TTS_STREAMING_DOC,
};
