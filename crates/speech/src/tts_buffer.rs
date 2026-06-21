//! Outbound TTS PCM buffer with flush support for barge-in.

use std::collections::VecDeque;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::Mutex;

use crate::pipeline::TtsAudioChunk;

/// Thread-safe queue of TTS PCM chunks awaiting injection to the outbound track.
#[derive(Clone, Default)]
pub struct TtsBuffer {
    inner: Arc<Mutex<TtsBufferInner>>,
}

#[derive(Default)]
struct TtsBufferInner {
    queue: VecDeque<TtsAudioChunk>,
    speaking: bool,
    flushed_generation: u64,
    generation: u64,
}

impl TtsBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn enqueue(&self, chunks: Vec<TtsAudioChunk>) {
        let _ = self.enqueue_if_generation(chunks, None).await;
    }

    /// Enqueue PCM only when the buffer generation still matches `expect_generation`.
    /// When `expect_generation` is `None`, always enqueues (legacy callers).
    /// Returns `true` when chunks were accepted.
    pub async fn enqueue_if_generation(
        &self,
        chunks: Vec<TtsAudioChunk>,
        expect_generation: Option<u64>,
    ) -> bool {
        let mut inner = self.inner.lock().await;
        if chunks.is_empty() {
            return false;
        }
        if let Some(expected) = expect_generation {
            if inner.generation != expected {
                return false;
            }
        }
        inner.speaking = true;
        inner.queue.extend(chunks);
        true
    }

    pub async fn flush(&self) -> u64 {
        let mut inner = self.inner.lock().await;
        inner.queue.clear();
        inner.speaking = false;
        inner.generation = inner.generation.wrapping_add(1);
        inner.flushed_generation = inner.generation;
        inner.generation
    }

    pub async fn pop_chunk(&self) -> Option<TtsAudioChunk> {
        let mut inner = self.inner.lock().await;
        let chunk = inner.queue.pop_front();
        if inner.queue.is_empty() {
            inner.speaking = false;
        }
        chunk
    }

    pub async fn is_speaking(&self) -> bool {
        self.inner.lock().await.speaking
    }

    pub async fn pending_count(&self) -> usize {
        self.inner.lock().await.queue.len()
    }

    pub async fn current_generation(&self) -> u64 {
        self.inner.lock().await.generation
    }

    pub async fn push_raw_pcm(&self, pcm: Bytes, duration_ms: u32) {
        self.enqueue(vec![TtsAudioChunk { pcm, duration_ms }]).await;
    }
}
