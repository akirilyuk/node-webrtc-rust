//! Speech pipeline errors.

use thiserror::Error;

/// Errors surfaced by the speech orchestration layer.
#[derive(Debug, Error)]
pub enum SpeechError {
    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("voice agent not attached")]
    NotAttached,

    #[error("voice agent already running")]
    AlreadyRunning,

    #[error("voice agent not running")]
    NotRunning,

    #[error("vendor error ({vendor}): {message}")]
    Vendor { vendor: String, message: String },

    #[error("VAD error: {0}")]
    Vad(String),

    #[error("TTS error: {0}")]
    Tts(String),

    #[error("STT error: {0}")]
    Stt(String),

    #[error("internal error: {0}")]
    Internal(String),
}

pub type SpeechResult<T> = Result<T, SpeechError>;
