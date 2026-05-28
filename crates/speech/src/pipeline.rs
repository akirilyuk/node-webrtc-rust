//! STT/TTS pipeline traits.

use async_trait::async_trait;
use bytes::Bytes;

use crate::config::{SttConfig, TtsConfig};
use crate::error::SpeechResult;

/// Chunk of synthesized PCM ready for outbound injection.
#[derive(Debug, Clone)]
pub struct TtsAudioChunk {
    pub pcm: Bytes,
    pub duration_ms: u32,
}

/// Streaming STT transcript update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttTranscript {
    Partial(String),
    Final(String),
}

/// Speech-to-text provider trait.
#[async_trait]
pub trait SttProvider: Send + Sync {
    fn vendor_name(&self) -> &'static str;

    async fn start(&mut self) -> SpeechResult<()>;

    async fn stop(&mut self) -> SpeechResult<()>;

    /// Feed mono PCM at the configured sample rate.
    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()>;

    /// Poll for the next transcript update, if any.
    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>>;

    /// Signal end-of-utterance to streaming STT vendors (e.g. Sherpa `input_finished`).
    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        Ok(())
    }
}

/// Text-to-speech provider trait.
#[async_trait]
pub trait TtsProvider: Send + Sync {
    fn vendor_name(&self) -> &'static str;

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>>;
}

/// Factory for constructing vendor providers from config.
pub trait VendorFactory: Send + Sync {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>>;
    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>>;
}
