//! Last-utterance PCM ring buffer for mid-session STT replay after `update_stt_config`.

use std::time::Instant;

use bytes::Bytes;

/// Maximum post-RNNoise mono 16 kHz PCM retained for replay (~480 KiB at 15 s).
pub const SPEECH_REPLAY_MAX_MS: u32 = 15_000;

/// Replay is skipped when the saved utterance is older than this (wall clock).
pub const SPEECH_REPLAY_MAX_AGE_MS: u64 = 10_000;

/// Bytes per millisecond of mono 16 kHz s16le PCM.
const BYTES_PER_MS_16K_MONO: usize = 32;

pub fn replay_max_bytes() -> usize {
    SPEECH_REPLAY_MAX_MS as usize * BYTES_PER_MS_16K_MONO
}

/// PCM captured between `user_speaking_start` and `finalize_stt_utterance`.
#[derive(Debug, Clone)]
pub struct UtteranceReplayBuffer {
    max_bytes: usize,
    pcm: Vec<u8>,
    overflow: bool,
    active: bool,
}

impl UtteranceReplayBuffer {
    pub fn new() -> Self {
        Self::with_max_bytes(replay_max_bytes())
    }

    pub fn with_max_bytes(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            pcm: Vec::new(),
            overflow: false,
            active: false,
        }
    }

    pub fn begin_utterance(&mut self) {
        self.pcm.clear();
        self.overflow = false;
        self.active = true;
    }

    pub fn end_utterance(&mut self) {
        self.active = false;
    }

    pub fn push(&mut self, chunk: &[u8]) {
        if !self.active || self.overflow {
            return;
        }
        let new_len = self.pcm.len() + chunk.len();
        if new_len > self.max_bytes {
            self.overflow = true;
            self.pcm.clear();
            self.active = false;
            return;
        }
        self.pcm.extend_from_slice(chunk);
    }

    /// True between `begin_utterance` and `end_utterance`.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Marks the buffer unusable (replay unavailable until the next `begin_utterance`).
    pub fn invalidate(&mut self) {
        self.pcm.clear();
        self.overflow = true;
        self.active = false;
    }

    pub fn overflowed(&self) -> bool {
        self.overflow
    }

    pub fn snapshot_pcm(&self) -> Option<Bytes> {
        if self.overflow || self.pcm.is_empty() {
            None
        } else {
            Some(Bytes::copy_from_slice(&self.pcm))
        }
    }
}

/// Saved utterance eligible for `replay_last_utterance`.
#[derive(Debug, Clone)]
pub struct UtteranceReplaySnapshot {
    pub pcm: Bytes,
    pub utterance_id: String,
    pub finalized_at: Instant,
    pub overflow: bool,
}

impl UtteranceReplaySnapshot {
    pub fn age_ms(&self) -> u64 {
        self.finalized_at.elapsed().as_millis() as u64
    }

    pub fn replayable(&self) -> bool {
        self.replayable_within(SPEECH_REPLAY_MAX_AGE_MS)
    }

    pub fn replayable_within(&self, max_age_ms: u64) -> bool {
        !self.overflow && !self.pcm.is_empty() && self.age_ms() <= max_age_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_clears_buffer() {
        let mut buf = UtteranceReplayBuffer::with_max_bytes(100);
        buf.begin_utterance();
        buf.push(&[0_u8; 50]);
        buf.push(&[0_u8; 60]);
        assert!(buf.overflowed());
        assert!(buf.snapshot_pcm().is_none());
    }
}
