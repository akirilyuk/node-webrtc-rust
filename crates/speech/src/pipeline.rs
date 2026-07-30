//! STT/TTS pipeline traits.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::mpsc::UnboundedSender;

use crate::config::{SttConfig, TtsConfig, VoiceSessionContext};
use crate::error::SpeechResult;

/// Chunk of synthesized PCM ready for outbound injection.
#[derive(Debug, Clone)]
pub struct TtsAudioChunk {
    pub pcm: Bytes,
    pub duration_ms: u32,
}

/// Progressive PCM sink for vendors that can emit audio during generation.
///
/// `cancel` is set by barge-in / flush so native generators can stop early.
#[derive(Clone)]
pub struct TtsProgressiveSink {
    pub tx: UnboundedSender<TtsAudioChunk>,
    pub cancel: Arc<AtomicBool>,
}

impl TtsProgressiveSink {
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn send(&self, chunk: TtsAudioChunk) -> bool {
        if self.is_cancelled() {
            return false;
        }
        self.tx.send(chunk).is_ok()
    }
}

/// Whether the agent should stream TTS chunks during synthesis (default **on**).
///
/// Set `VOICE_TTS_STREAM_CHUNKS=0` (also `false` / `off` / `no`) to keep the legacy
/// buffered path: fully synthesize, then enqueue, then drain.
pub fn tts_stream_chunks_enabled() -> bool {
    match std::env::var("VOICE_TTS_STREAM_CHUNKS")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("0") | Some("false") | Some("no") | Some("off") => false,
        _ => true,
    }
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

    /// Optional hook when a voice session starts (e.g. Sherpa phrase-cache project scope).
    fn bind_session_context(&self, _ctx: &VoiceSessionContext) {}

    /// Fully synthesize `text` and return all PCM chunks (legacy / cache-friendly path).
    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>>;

    /// Synthesize with optional progressive delivery.
    ///
    /// Default: call [`Self::synthesize`] then send each returned chunk on `sink` (if any).
    /// Streaming vendors (e.g. Sherpa) override to emit deltas during ONNX generate.
    /// Always returns the full utterance chunks when generation completes successfully.
    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        let chunks = self.synthesize(text).await?;
        if let Some(sink) = sink {
            for chunk in &chunks {
                if !sink.send(chunk.clone()) {
                    break;
                }
            }
        }
        Ok(chunks)
    }
}

/// Factory for constructing vendor providers from config.
pub trait VendorFactory: Send + Sync {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>>;
    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>>;
}
