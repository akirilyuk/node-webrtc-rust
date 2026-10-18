mod factory;
#[cfg(feature = "live")]
mod http;
mod matrix;
mod stt;
mod tts;

pub use factory::AzureFactory;
pub use matrix::{
    default_stt_mode, default_tts_voice, validate_stt_mode, validate_tts_voice, CHUNK_IDLE_TIMEOUT,
    CONNECT_TIMEOUT, DOCUMENTED_STT_RECOGNITION_MODES, DOCUMENTED_TTS_VOICES, FIRST_AUDIO_TIMEOUT,
    HTTP_TOTAL_TIMEOUT, POOL_IDLE_TIMEOUT, STT_SHORT_AUDIO_DOC, TCP_KEEPALIVE,
    TTS_LANGUAGE_SUPPORT_DOC, TTS_REST_DOC, TTS_VOICES_LIST_DOC,
};
pub use stt::AzureStt;
pub use tts::AzureTts;
