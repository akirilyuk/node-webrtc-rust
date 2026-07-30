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
    /// True while a synthesis job may still push more chunks (progressive TTS).
    /// Keeps drain from treating an empty queue as end-of-utterance mid-synth.
    producing: bool,
    /// Incomplete stereo PCM (< one 20 ms frame) held across progressive drain passes.
    /// Padding this mid-utterance would insert silence and hurt STT quality.
    frame_carry: Vec<u8>,
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

    /// Mark that synthesis may still produce chunks. While producing, an empty
    /// queue is not treated as idle/end-of-utterance.
    pub async fn set_producing(&self, producing: bool) {
        let mut inner = self.inner.lock().await;
        inner.producing = producing;
        if producing {
            inner.speaking = true;
        } else if inner.queue.is_empty() {
            inner.speaking = false;
        }
    }

    pub async fn is_producing(&self) -> bool {
        self.inner.lock().await.producing
    }

    pub async fn flush(&self) -> u64 {
        let mut inner = self.inner.lock().await;
        inner.queue.clear();
        inner.frame_carry.clear();
        inner.speaking = false;
        inner.producing = false;
        inner.generation = inner.generation.wrapping_add(1);
        inner.flushed_generation = inner.generation;
        inner.generation
    }

    /// Take any incomplete frame bytes left from a prior drain pass.
    pub async fn take_frame_carry(&self) -> Vec<u8> {
        let mut inner = self.inner.lock().await;
        std::mem::take(&mut inner.frame_carry)
    }

    /// Persist incomplete frame bytes until the next drain pass (or flush).
    pub async fn store_frame_carry(&self, carry: Vec<u8>) {
        let mut inner = self.inner.lock().await;
        inner.frame_carry = carry;
    }

    pub async fn pop_chunk(&self) -> Option<TtsAudioChunk> {
        let mut inner = self.inner.lock().await;
        let chunk = inner.queue.pop_front();
        if inner.queue.is_empty() && !inner.producing {
            inner.speaking = false;
        }
        chunk
    }

    /// True when PCM is queued **or** synthesis is still producing chunks.
    pub async fn is_speaking(&self) -> bool {
        let inner = self.inner.lock().await;
        inner.speaking || inner.producing
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

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(duration_ms: u32) -> TtsAudioChunk {
        TtsAudioChunk {
            pcm: Bytes::from(vec![0_u8; 3840]),
            duration_ms,
        }
    }

    #[tokio::test]
    async fn producing_keeps_speaking_when_queue_empty() {
        let buf = TtsBuffer::new();
        buf.set_producing(true).await;
        assert!(buf.is_speaking().await);
        assert!(buf.is_producing().await);
        assert!(buf.pop_chunk().await.is_none());
        assert!(buf.is_speaking().await);
        buf.set_producing(false).await;
        assert!(!buf.is_speaking().await);
    }

    #[tokio::test]
    async fn flush_clears_producing() {
        let buf = TtsBuffer::new();
        buf.set_producing(true).await;
        buf.enqueue(vec![chunk(20)]).await;
        let gen = buf.flush().await;
        assert_eq!(gen, 1);
        assert!(!buf.is_producing().await);
        assert!(!buf.is_speaking().await);
        assert_eq!(buf.pending_count().await, 0);
    }

    #[tokio::test]
    async fn flush_clears_frame_carry() {
        let buf = TtsBuffer::new();
        buf.store_frame_carry(vec![1, 2, 3]).await;
        assert_eq!(buf.take_frame_carry().await, vec![1, 2, 3]);
        buf.store_frame_carry(vec![9]).await;
        let _ = buf.flush().await;
        assert!(buf.take_frame_carry().await.is_empty());
    }
}
