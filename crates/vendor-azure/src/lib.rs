mod factory;
mod matrix;
mod stt;
mod tts;

pub use factory::AzureFactory;
pub use matrix::{
    default_stt_mode, default_tts_voice, validate_stt_mode, validate_tts_voice,
    DOCUMENTED_STT_RECOGNITION_MODES, DOCUMENTED_TTS_VOICES, STT_SHORT_AUDIO_DOC, TTS_REST_DOC,
};
