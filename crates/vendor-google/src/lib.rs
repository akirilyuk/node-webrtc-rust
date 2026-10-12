#[cfg(feature = "live")]
mod auth;
mod factory;
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
    GoogleSttV2Locator, SttDefaultTransport, DOCUMENTED_STT_MODELS, STT_V2_RECOGNIZER_DOC,
    STT_V2_STREAMING_DOC, TTS_STREAMING_DOC,
};
