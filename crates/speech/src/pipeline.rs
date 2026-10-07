//! STT/TTS pipeline traits.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::mpsc::UnboundedSender;

use crate::config::{LanguageIdConfig, SttConfig, TtsConfig, VoiceSessionContext};
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

    /// Push audio and collect every transcript that is ready, in order. Default: push_audio, then
    /// poll_transcript until None. Vendors that can do both in one blocking hop override it.
    async fn push_and_poll(
        &mut self,
        pcm: Bytes,
        out: &mut Vec<SttTranscript>,
    ) -> SpeechResult<()> {
        self.push_audio(pcm).await?;
        while let Some(t) = self.poll_transcript().await? {
            out.push(t);
        }
        Ok(())
    }

    /// Wait until the provider can transcribe (its stream is open and ready). Returns false on timeout.
    /// Default: always ready.
    async fn wait_ready(&mut self, _timeout: std::time::Duration) -> SpeechResult<bool> {
        Ok(true)
    }

    /// Signal end-of-utterance to streaming STT vendors (e.g. Sherpa `input_finished`).
    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    /// Optional session labels for remote STT (cluster-sherpa gRPC metadata).
    fn bind_session_context(&self, _ctx: &VoiceSessionContext) {}

    /// Milliseconds of audio accepted by [`Self::push_audio`] and not yet processed by
    /// [`Self::poll_transcript`] (including time blocked on a shared decode limiter). Used to
    /// defer C1 `user_stt_not_found` when the decoder is behind.
    fn decode_backlog_ms(&self) -> u32 {
        0
    }

    /// `true` from the start of a stream open until the server answers Ready, including while a
    /// refused open is retried (e.g. a speech pod at its stream cap). Audio stays queued;
    /// VoiceAgent waits instead of ending the turn (C1).
    fn stream_open_pending(&self) -> bool {
        false
    }

    /// Drop transcripts already queued inside the provider (results of audio pushed before this
    /// call). VoiceAgent calls it when a new utterance starts, so a late result of the previous
    /// utterance can never be emitted as the new one. Returns how many were dropped.
    fn discard_queued_transcripts(&mut self) -> usize {
        0
    }
}

/// A synthesis plus whether it ran to the end (not cancelled, not cut short).
#[derive(Debug, Clone, Default)]
pub struct TtsSynthesis {
    pub chunks: Vec<TtsAudioChunk>,
    pub complete: bool,
}

/// A synthesis start that was refused and retried before it succeeded or gave up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TtsOpenWait {
    /// Milliseconds spent waiting on refusals.
    pub wait_ms: u32,
    /// Number of refused attempts that were retried.
    pub attempts: u32,
    /// Last refusal, e.g. `UNAVAILABLE: speech models are still loading`.
    pub reason: String,
}

/// Text-to-speech provider trait.
#[async_trait]
pub trait TtsProvider: Send + Sync {
    fn vendor_name(&self) -> &'static str;

    /// Optional hook when a voice session starts (e.g. Sherpa phrase-cache project scope).
    fn bind_session_context(&self, _ctx: &VoiceSessionContext) {}

    /// Takes the open-wait record of the last synthesis if its start was refused and retried
    /// (see [`TtsOpenWait`]). Default: vendors that never wait return `None`.
    fn take_open_wait(&self) -> Option<TtsOpenWait> {
        None
    }

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

    /// Like [`Self::synthesize_progressive`] and reports completeness. Only complete
    /// syntheses may be cached. Default: complete unless the sink was cancelled.
    async fn synthesize_progressive_with_status(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<TtsSynthesis> {
        let cancelled = sink.clone();
        let chunks = self.synthesize_progressive(text, sink).await?;
        let complete = !cancelled
            .as_ref()
            .is_some_and(TtsProgressiveSink::is_cancelled);
        Ok(TtsSynthesis { chunks, complete })
    }
}

/// Result from offline spoken-language identification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageIdResult {
    pub language: String,
}

/// Offline spoken-language identification (e.g. Sherpa Whisper tiny).
#[async_trait]
pub trait LanguageIdProvider: Send + Sync {
    /// Identify language from mono PCM at `sample_rate` Hz (typically 16_000).
    async fn identify(
        &self,
        pcm: Bytes,
        sample_rate: u32,
    ) -> SpeechResult<Option<LanguageIdResult>>;
}

/// Factory for constructing vendor providers from config.
pub trait VendorFactory: Send + Sync {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>>;
    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>>;
    fn create_language_id(
        &self,
        _config: &LanguageIdConfig,
    ) -> SpeechResult<Option<Box<dyn LanguageIdProvider>>> {
        Ok(None)
    }
}
