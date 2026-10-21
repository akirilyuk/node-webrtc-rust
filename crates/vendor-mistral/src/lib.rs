mod client;
mod error;
mod factory;
mod matrix;
mod stt;
mod stt_matrix;
mod tts;
mod wav;

pub use factory::MistralFactory;
pub use matrix::{
    implemented_tts_transports, resolve_base_url, resolve_base_url_from_env, tts_url,
    validate_tts_model, Timeouts, BASE_URL_ENV, CHUNK_IDLE_TIMEOUT, CONNECT_TIMEOUT,
    DEFAULT_BASE_URL, DEFAULT_TTS_MODEL, DEFAULT_TTS_VOICE, DOCUMENTED_TTS_MODELS, ENDPOINTS,
    FIRST_AUDIO_TIMEOUT, GENERATE_SPEECH_DOC, HTTP_TOTAL_TIMEOUT, POOL_IDLE_TIMEOUT,
    REGIONAL_INFERENCE_DOC, STREAM_SPEECH_DOC, TCP_KEEPALIVE, TTS_REALTIME_MODEL_DOC,
    TTS_SAMPLE_RATE, VOICES_DOC,
};
pub use stt::MistralStt;
pub use stt_matrix::{
    realtime_stt_url, session_language, validate_streaming_delay, validate_stt_model, SttTimeouts,
    APPEND_MAX_BYTES, APPEND_TARGET_BYTES, DEFAULT_STT_MODEL, DOCUMENTED_STT_MODELS,
    STREAMING_DELAY_MS, STT_CLOSE_TIMEOUT, STT_CONNECT_TIMEOUT, STT_DOC, STT_DONE_TIMEOUT,
    STT_SAMPLE_RATE,
};
pub use tts::MistralTts;

/// Bench-only knobs, not public API: the live bench measures other `target_streaming_delay_ms`
/// values than the production default.
#[cfg(feature = "live")]
#[doc(hidden)]
pub mod __test {
    use node_webrtc_rust_speech::config::SttConfig;
    use node_webrtc_rust_speech::error::SpeechResult;

    /// A [`crate::MistralStt`] sending `target_streaming_delay_ms = delay_ms` (80 to 1200, or
    /// 2400; anything else is a config error).
    pub fn stt_with_streaming_delay(
        config: &SttConfig,
        delay_ms: u32,
    ) -> SpeechResult<crate::MistralStt> {
        crate::MistralStt::with_streaming_delay(config, delay_ms)
    }
}
