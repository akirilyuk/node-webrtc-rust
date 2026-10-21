mod client;
mod error;
mod factory;
mod matrix;
mod tts;
mod wav;

pub use factory::MistralFactory;
pub use matrix::{
    tts_url, validate_tts_model, Timeouts, CHUNK_IDLE_TIMEOUT, CONNECT_TIMEOUT, DEFAULT_BASE_URL,
    DEFAULT_TTS_MODEL, DEFAULT_TTS_VOICE, DOCUMENTED_TTS_MODELS, FIRST_AUDIO_TIMEOUT,
    GENERATE_SPEECH_DOC, HTTP_TOTAL_TIMEOUT, POOL_IDLE_TIMEOUT, STREAM_SPEECH_DOC, TCP_KEEPALIVE,
    TTS_SAMPLE_RATE, VOICES_DOC,
};
pub use tts::MistralTts;
