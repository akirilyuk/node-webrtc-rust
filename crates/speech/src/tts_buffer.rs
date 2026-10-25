//! Outbound TTS PCM buffer with flush support for barge-in.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::Mutex;

use crate::events::SpeechEvent;
use crate::pipeline::TtsAudioChunk;

/// Outbound PCM is 48 kHz stereo s16le: 192 bytes per millisecond.
const PCM_BYTES_PER_MS: u64 = 192;

/// Bytes of one reply that were queued and played, in queue order (see [`TtsBuffer::consume_played`]).
struct ReplyLedgerEntry {
    reply_id: Arc<str>,
    queued_bytes: u64,
    played_bytes: u64,
    /// Some of this reply's bytes have been written to the outbound track.
    started: bool,
    /// No more bytes will be queued for this reply.
    complete: bool,
    /// Speak request to the first PCM chunk from the TTS vendor.
    first_chunk_ms: Option<u32>,
}

/// A reply boundary crossed while PCM was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyTransition {
    /// The first byte of the reply reached the outbound track.
    Started {
        id: Arc<str>,
        first_chunk_ms: Option<u32>,
    },
    /// The last byte of a complete reply reached the outbound track.
    /// An ended reply is always complete: `synthesized_ms` equals what was queued.
    Ended {
        id: Arc<str>,
        played_ms: u32,
        synthesized_ms: u32,
    },
}

/// A reply dropped by [`TtsBuffer::flush`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlushedReply {
    pub id: Arc<str>,
    pub played_ms: u32,
    pub synthesized_ms: u32,
    pub synthesis_complete: bool,
}

fn played_ms(played_bytes: u64) -> u32 {
    u32::try_from(played_bytes / PCM_BYTES_PER_MS).unwrap_or(u32::MAX)
}

/// Thread-safe queue of TTS PCM chunks awaiting injection to the outbound track.
#[derive(Clone, Default)]
pub struct TtsBuffer {
    inner: Arc<Mutex<TtsBufferInner>>,
    /// True from the moment a chunk is handed to the drain worker through a [`TtsDrainHold`]
    /// until that drain pass has finished writing it. Without it, `speaking` is false in the gap
    /// between "last chunk popped" and "first frame written", so idle checks (blocking send,
    /// `wait_tts_playback_idle`) can return before any PCM is written.
    held: Arc<AtomicBool>,
}

/// One drain pass. Chunks popped through it count as in flight until it is dropped, so the
/// hold cannot outlive the pass that owns the chunks (a plain [`TtsBuffer::pop_chunk`] never
/// sets it).
pub struct TtsDrainHold(TtsBuffer);

impl TtsDrainHold {
    /// Pop the next chunk and keep the buffer "speaking" until this guard is dropped.
    pub async fn pop_chunk(&self) -> Option<TtsAudioChunk> {
        self.0.pop_chunk_marking(true).await
    }
}

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
    /// Replies (tagged enqueues only) in queue order, with bytes queued and played. Untagged
    /// audio creates no entry.
    reply_ledger: VecDeque<ReplyLedgerEntry>,
    /// Ledger entries dropped by [`TtsBuffer::flush`], until [`TtsBuffer::take_flushed_replies`].
    flushed_replies: Vec<FlushedReply>,
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
        self.enqueue_tagged_if_generation(chunks, expect_generation, None)
            .await
    }

    /// Like [`enqueue`](Self::enqueue), and books the chunks' PCM bytes to a reply in the ledger.
    /// `reply` is the reply id and, when the entry is created, the speak request to first vendor
    /// chunk time.
    pub async fn enqueue_tagged(
        &self,
        chunks: Vec<TtsAudioChunk>,
        reply: Option<(&Arc<str>, Option<u32>)>,
    ) {
        let _ = self.enqueue_tagged_if_generation(chunks, None, reply).await;
    }

    /// [`enqueue_if_generation`](Self::enqueue_if_generation) with a reply tag. A tag adds the
    /// bytes to the tail entry when its id matches (and it is not complete), else opens a new one.
    pub async fn enqueue_tagged_if_generation(
        &self,
        chunks: Vec<TtsAudioChunk>,
        expect_generation: Option<u64>,
        reply: Option<(&Arc<str>, Option<u32>)>,
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
        if let Some((id, first_chunk_ms)) = reply {
            let bytes: u64 = chunks.iter().map(|c| c.pcm.len() as u64).sum();
            if bytes > 0 {
                match inner.reply_ledger.back_mut() {
                    Some(tail) if !tail.complete && tail.reply_id == *id => {
                        tail.queued_bytes += bytes;
                    }
                    _ => inner.reply_ledger.push_back(ReplyLedgerEntry {
                        reply_id: Arc::clone(id),
                        queued_bytes: bytes,
                        played_bytes: 0,
                        started: false,
                        complete: false,
                        first_chunk_ms,
                    }),
                }
            }
        }
        inner.queue.extend(chunks);
        true
    }

    /// No more bytes will be queued for reply `id`. The end transition is reported by the
    /// [`consume_played`](Self::consume_played) call that finds the reply fully played (the drain
    /// calls it with `0` bytes after a pass, so a reply that finished playing first still ends).
    pub async fn mark_reply_complete(&self, id: &str) {
        let mut inner = self.inner.lock().await;
        if let Some(entry) = inner
            .reply_ledger
            .iter_mut()
            .rev()
            .find(|e| &*e.reply_id == id)
        {
            entry.complete = true;
        }
    }

    /// Book `bytes` of PCM written to the outbound track against the ledger, oldest reply first.
    /// Bytes carried across frames and chunks stay in order, so a frame holding the tail of one
    /// reply and the head of the next is split between them. Bytes beyond the tagged entries
    /// (untagged audio) are ignored. An entry ends only when it is complete and fully played, so
    /// a progressive underrun never ends a reply early. `0` only reports finished replies.
    pub async fn consume_played(&self, bytes: u64) -> Vec<ReplyTransition> {
        let mut inner = self.inner.lock().await;
        let mut remaining = bytes;
        let mut out = Vec::new();
        while let Some(head) = inner.reply_ledger.front_mut() {
            let take = remaining.min(head.queued_bytes - head.played_bytes);
            if take > 0 {
                if !head.started {
                    head.started = true;
                    out.push(ReplyTransition::Started {
                        id: Arc::clone(&head.reply_id),
                        first_chunk_ms: head.first_chunk_ms,
                    });
                }
                head.played_bytes += take;
                remaining -= take;
            }
            if head.complete && head.started && head.played_bytes >= head.queued_bytes {
                out.push(ReplyTransition::Ended {
                    id: Arc::clone(&head.reply_id),
                    played_ms: played_ms(head.played_bytes),
                    synthesized_ms: played_ms(head.queued_bytes),
                });
                inner.reply_ledger.pop_front();
                continue;
            }
            break;
        }
        out
    }

    /// Replies dropped by [`flush`](Self::flush), in queue order, with the ms of each that was
    /// played. Clears the list.
    pub async fn take_flushed_replies(&self) -> Vec<FlushedReply> {
        std::mem::take(&mut self.inner.lock().await.flushed_replies)
    }

    /// [`flush`](Self::flush), then `emit` `agent_reply_end { interrupted: true }` for every
    /// reply it dropped.
    pub async fn flush_with_reply_events(&self, emit: impl Fn(SpeechEvent)) -> u64 {
        let generation = self.flush().await;
        for r in self.take_flushed_replies().await {
            emit(SpeechEvent::agent_reply_end(
                &r.id,
                r.played_ms,
                true,
                r.synthesized_ms,
                r.synthesis_complete,
            ));
        }
        generation
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
        let dropped: Vec<FlushedReply> = inner
            .reply_ledger
            .drain(..)
            .map(|e| FlushedReply {
                id: e.reply_id,
                played_ms: played_ms(e.played_bytes),
                synthesized_ms: played_ms(e.queued_bytes),
                synthesis_complete: e.complete,
            })
            .collect();
        inner.flushed_replies.extend(dropped);
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
        self.pop_chunk_marking(false).await
    }

    async fn pop_chunk_marking(&self, hold: bool) -> Option<TtsAudioChunk> {
        let mut inner = self.inner.lock().await;
        let chunk = inner.queue.pop_front();
        // Clear `speaking` only when the drain worker finds nothing left to pop. Clearing it
        // when the last chunk is handed out opens a gap (chunk popped, first frame not yet
        // written, `agent_speaking` still false) in which a blocking send sees "idle" and
        // returns before playback starts.
        if chunk.is_none() && !inner.producing {
            inner.speaking = false;
        }
        // Set under the buffer lock so `is_speaking` never sees the chunk in neither place.
        if hold && chunk.is_some() {
            self.held.store(true, Ordering::SeqCst);
        }
        chunk
    }

    /// Guard for one drain pass: chunks popped through it count as in flight even when the
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
        assert!(hold.pop_chunk().await.is_some());
        buf.set_producing(false).await;
        assert!(buf.is_speaking().await, "chunk in hand must not look idle");
        drop(hold);
        assert!(!buf.is_speaking().await);
    }

    #[tokio::test]
    async fn flush_releases_held_chunk() {
        let buf = TtsBuffer::new();
        buf.enqueue(vec![chunk(20)]).await;
        let hold = buf.drain_hold();
        let _ = hold.pop_chunk().await;
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

    fn tagged(bytes: usize) -> Vec<TtsAudioChunk> {
        vec![TtsAudioChunk {
            pcm: Bytes::from(vec![0_u8; bytes]),
            duration_ms: 0,
        }]
    }

    #[tokio::test]
    async fn ledger_splits_a_frame_between_two_replies() {
        let buf = TtsBuffer::new();
        let a: Arc<str> = Arc::from("a");
        let b: Arc<str> = Arc::from("b");
        buf.enqueue_tagged(tagged(5000), Some((&a, Some(10)))).await;
        buf.enqueue_tagged(tagged(2680), Some((&b, Some(20)))).await;
        buf.mark_reply_complete("a").await;
        buf.mark_reply_complete("b").await;
        // Two 3840-byte frames: the first is all of A's head, the second holds A's tail
        // (1160 bytes) and B's head (2680 bytes).
        let first = buf.consume_played(3840).await;
        assert_eq!(
            first,
            vec![ReplyTransition::Started {
                id: a.clone(),
                first_chunk_ms: Some(10)
            }]
        );
        let second = buf.consume_played(3840).await;
        assert_eq!(
            second,
            vec![
                ReplyTransition::Ended {
                    id: a.clone(),
                    played_ms: 26,
                    synthesized_ms: 26
                },
                ReplyTransition::Started {
                    id: b.clone(),
                    first_chunk_ms: Some(20)
                },
                ReplyTransition::Ended {
                    id: b.clone(),
                    played_ms: 13,
                    synthesized_ms: 13
                },
            ]
        );
    }

    #[tokio::test]
    async fn ledger_does_not_end_an_incomplete_reply() {
        let buf = TtsBuffer::new();
        let a: Arc<str> = Arc::from("a");
        buf.enqueue_tagged(tagged(3840), Some((&a, None))).await;
        let t = buf.consume_played(3840).await;
        assert_eq!(t.len(), 1, "start only while the reply is still producing");
        buf.enqueue_tagged(tagged(3840), Some((&a, None))).await;
        assert!(buf.consume_played(3840).await.is_empty());
        buf.mark_reply_complete("a").await;
        let end = buf.consume_played(0).await;
        assert_eq!(
            end,
            vec![ReplyTransition::Ended {
                id: a,
                played_ms: 40,
                synthesized_ms: 40
            }]
        );
    }

    #[tokio::test]
    async fn untagged_audio_creates_no_ledger_entry() {
        let buf = TtsBuffer::new();
        buf.push_raw_pcm(Bytes::from(vec![0_u8; 3840]), 20).await;
        assert!(buf.consume_played(3840).await.is_empty());
        assert!(buf.take_flushed_replies().await.is_empty());
    }

    #[tokio::test]
    async fn flush_reports_every_ledger_entry_with_played_ms() {
        let buf = TtsBuffer::new();
        let a: Arc<str> = Arc::from("a");
        let b: Arc<str> = Arc::from("b");
        buf.enqueue_tagged(tagged(19_200), Some((&a, None))).await;
        buf.enqueue_tagged(tagged(3840), Some((&b, None))).await;
        let _ = buf.consume_played(9_600).await;
        buf.flush().await;
        let flushed = buf.take_flushed_replies().await;
        assert_eq!(
            flushed,
            vec![
                FlushedReply {
                    id: a,
                    played_ms: 50,
                    synthesized_ms: 100,
                    synthesis_complete: false
                },
                FlushedReply {
                    id: b,
                    played_ms: 0,
                    synthesized_ms: 20,
                    synthesis_complete: false
                }
            ]
        );
        assert!(buf.take_flushed_replies().await.is_empty());
        assert!(buf.consume_played(3840).await.is_empty());
    }

    #[tokio::test]
    async fn flush_mid_reply_reports_synthesized_and_played_ms() {
        let buf = TtsBuffer::new();
        let a: Arc<str> = Arc::from("a");
        buf.enqueue_tagged(tagged(38_400), Some((&a, None))).await;
        buf.enqueue_tagged(tagged(38_400), Some((&a, None))).await;
        let _ = buf.consume_played(19_200).await;
        buf.flush().await;
        assert_eq!(
            buf.take_flushed_replies().await,
            vec![FlushedReply {
                id: a,
                played_ms: 100,
                synthesized_ms: 400,
                synthesis_complete: false
            }]
        );
    }

    #[tokio::test]
    async fn flush_of_complete_reply_reports_synthesis_complete() {
        let buf = TtsBuffer::new();
        let a: Arc<str> = Arc::from("a");
        buf.enqueue_tagged(tagged(38_400), Some((&a, None))).await;
        buf.enqueue_tagged(tagged(38_400), Some((&a, None))).await;
        buf.mark_reply_complete("a").await;
        let _ = buf.consume_played(19_200).await;
        buf.flush().await;
        assert_eq!(
            buf.take_flushed_replies().await,
            vec![FlushedReply {
                id: a,
                played_ms: 100,
                synthesized_ms: 400,
                synthesis_complete: true
            }]
        );
    }

    #[tokio::test]
    async fn ended_reply_reports_synthesized_equal_to_played() {
        let buf = TtsBuffer::new();
        let a: Arc<str> = Arc::from("a");
        buf.enqueue_tagged(tagged(38_400), Some((&a, None))).await;
        buf.mark_reply_complete("a").await;
        let t = buf.consume_played(38_400).await;
        assert_eq!(
            t.last(),
            Some(&ReplyTransition::Ended {
                id: a,
                played_ms: 200,
                synthesized_ms: 200
            })
        );
    }
}
