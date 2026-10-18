mod factory;
mod matrix;
mod stt;
mod tts;

pub use factory::AwsFactory;
pub use matrix::{
    default_stt_language_code, default_tts_voice, validate_stt_language_code, validate_tts_voice,
    CHUNK_IDLE_TIMEOUT, CONNECT_TIMEOUT, DOCUMENTED_STT_LANGUAGE_CODES, DOCUMENTED_TTS_VOICES,
    FIRST_AUDIO_TIMEOUT, POLLY_OPERATION_TIMEOUT, POLLY_SYNTH_DOC, TRANSCRIBE_STREAMING_DOC,
};
pub use stt::AwsStt;
pub use tts::AwsTts;
