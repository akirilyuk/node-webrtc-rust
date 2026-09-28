mod factory;
mod matrix;
mod stt;
mod tts;

pub use factory::AwsFactory;
pub use matrix::{
    default_stt_language_code, default_tts_voice, validate_stt_language_code, validate_tts_voice,
    DOCUMENTED_STT_LANGUAGE_CODES, DOCUMENTED_TTS_VOICES, POLLY_SYNTH_DOC,
    TRANSCRIBE_STREAMING_DOC,
};
