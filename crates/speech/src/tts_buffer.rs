//! Outbound TTS PCM buffer with flush support for barge-in.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::Mutex;

use crate::pipeline::TtsAudioChunk;

/// Thread-safe queue of TTS PCM chunks awaiting injection to the outbound track.
#[derive(Clone, Default)]
pub struct TtsBuffer {
    inner: Arc<Mutex<TtsBufferInner>>,
    /// True from the moment a chunk is handed to the drain worker until that drain pass has
    /// finished writing it (see [`TtsDrainHold`]). Without it, `speaking` is false in the gap
    /// between "last chunk popped" and "first frame written", so idle checks (blocking send,
    /// `wait_tts_playback_idle`) can return before any PCM is written.
    held: Arc<AtomicBool>,
}

/// Drop guard that marks the chunk(s) a drain pass holds as finished.
pub struct TtsDrainHold(TtsBuffer);

impl Drop for TtsDrainHold {
    fn drop(&mut self) {
        self.0.held.store(false, Ordering::SeqCst);
    }
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
        self.held.store(false, Ordering::SeqCst);
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
        // Clear `speaking` only when the drain worker finds nothing left to pop. Clearing it
        // when the last chunk is handed out opens a gap (chunk popped, first frame not yet
        // written, `agent_speaking` still false) in which a blocking send sees "idle" and
        // returns before playback starts.
        if chunk.is_none() && !inner.producing {
            inner.speaking = false;
        }
        if chunk.is_some() {
            self.held.store(true, Ordering::SeqCst);
        }
        chunk
    }

    /// Guard for one drain pass: while alive, popped chunks count as in flight even when the
    /// queue is empty and nothing is producing. Dropping it releases them.
    pub fn drain_hold(&self) -> TtsDrainHold {
        TtsDrainHold(self.clone())
    }

    /// True when PCM is queued, synthesis is still producing chunks, **or** the drain worker
    /// holds a popped chunk it has not finished writing.
    pub async fn is_speaking(&self) -> bool {
        let inner = self.inner.lock().await;
        inner.speaking || inner.producing || self.held.load(Ordering::SeqCst)
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

    /// Handing out the last chunk must not look idle until the drain worker has finished it
    /// (next pop finds nothing), or a blocking send can return before playback starts.
    #[tokio::test]
    async fn last_popped_chunk_keeps_speaking_until_next_pop() {
        let buf = TtsBuffer::new();
        buf.enqueue(vec![chunk(20)]).await;
        assert!(buf.pop_chunk().await.is_some());
        assert!(buf.is_speaking().await);
        assert!(buf.pop_chunk().await.is_none());
        assert!(!buf.is_speaking().await);
    }

    /// Gap: last chunk popped, `set_producing(false)` (synthesis done), first frame not yet
    /// written. The buffer must still report speaking until the drain pass releases its hold.
    #[tokio::test]
    async fn popped_chunk_in_hand_keeps_speaking_until_hold_released() {
        let buf = TtsBuffer::new();
        buf.set_producing(true).await;
        buf.enqueue(vec![chunk(20)]).await;
        let hold = buf.drain_hold();
        assert!(buf.pop_chunk().await.is_some());
        buf.set_producing(false).await;
        assert!(buf.is_speaking().await, "chunk in hand must not look idle");
        drop(hold);
        assert!(!buf.is_speaking().await);
    }

    #[tokio::test]
    async fn flush_releases_held_chunk() {
        let buf = TtsBuffer::new();
        buf.enqueue(vec![chunk(20)]).await;
        let _hold = buf.drain_hold();
        let _ = buf.pop_chunk().await;
        buf.flush().await;
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
        buf.flush().await;
        assert!(buf.take_frame_carry().await.is_empty());
    }
}
