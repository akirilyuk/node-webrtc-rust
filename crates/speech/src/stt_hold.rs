//! Host-controlled STT hold used for mid-session STT swaps that take seconds (cold model pools).
//!
//! While a hold is active the agent stops feeding the current STT and stops polling it, so no
//! transcript from the old model reaches the host. Inbound PCM that would have gone to STT is
//! kept in a bounded buffer and later decoded by the new STT (`release`) or the old one
//! (`cancel`).

use serde::{Deserialize, Serialize};

/// Bytes per millisecond of mono 16 kHz s16le PCM.
const BYTES_PER_MS: usize = 32;

/// Default hold buffer length (ms).
pub const STT_HOLD_DEFAULT_MAX_BUFFER_MS: u32 = 45_000;
/// Upper bound for `max_buffer_ms`.
pub const STT_HOLD_MAX_BUFFER_MS_CAP: u32 = 120_000;

/// Default replay max age (ms) for `replay_last_utterance`.
pub const REPLAY_DEFAULT_MAX_AGE_MS: u32 = 10_000;
/// Upper bound for `replay.max_age_ms`.
pub const REPLAY_MAX_AGE_MS_CAP: u32 = 120_000;

/// What the hold keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttHoldMode {
    /// Keep all STT-bound PCM from the start of the triggering utterance until release.
    BufferReplay,
    /// Keep only the triggering utterance; later speech is dropped (and counted).
    FirstUtterance,
}

impl SttHoldMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BufferReplay => "buffer_replay",
            Self::FirstUtterance => "first_utterance",
        }
    }
}

/// Options for [`crate::VoiceAgent::begin_stt_hold`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginSttHoldOptions {
    pub mode: SttHoldMode,
    /// Buffer bound in ms. Default 45 000, capped at 120 000.
    #[serde(default)]
    pub max_buffer_ms: Option<u32>,
}

impl Default for BeginSttHoldOptions {
    fn default() -> Self {
        Self {
            mode: SttHoldMode::BufferReplay,
            max_buffer_ms: None,
        }
    }
}

/// Options for [`crate::VoiceAgent::release_stt_hold`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseSttHoldOptions {
    /// Decode the held audio through the (new) STT. `false` drops the buffer.
    #[serde(default)]
    pub replay: bool,
}

/// `replay` section of `VoiceAgentConfig`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayConfig {
    /// Max age (ms) of the last finalized utterance for `replay_last_utterance`.
    /// Default 10 000, capped at 120 000.
    #[serde(default)]
    pub max_age_ms: Option<u32>,
}

pub fn resolved_replay_max_age_ms(config: &ReplayConfig) -> u64 {
    u64::from(
        config
            .max_age_ms
            .unwrap_or(REPLAY_DEFAULT_MAX_AGE_MS)
            .clamp(1, REPLAY_MAX_AGE_MS_CAP),
    )
}

pub fn resolved_hold_max_buffer_ms(max_buffer_ms: Option<u32>) -> u32 {
    max_buffer_ms
        .unwrap_or(STT_HOLD_DEFAULT_MAX_BUFFER_MS)
        .clamp(1, STT_HOLD_MAX_BUFFER_MS_CAP)
}

pub fn pcm_bytes_to_ms(bytes: usize) -> u32 {
    (bytes / BYTES_PER_MS) as u32
}

/// Active hold state (owned by the agent).
#[derive(Debug)]
pub struct SttHold {
    pub mode: SttHoldMode,
    max_bytes: usize,
    pcm: Vec<u8>,
    dropped_bytes: u64,
    /// Utterance this hold replaces (the in-progress one at begin, else the first one seen).
    pub replaces_utterance_id: Option<String>,
    /// `first_utterance`: the kept utterance has ended; later audio is dropped.
    pub first_closed: bool,
    /// An utterance finalize was requested while held (the old STT never saw it).
    pub finalize_seen: bool,
    ever_had_audio: bool,
}

impl SttHold {
    pub fn new(mode: SttHoldMode, max_buffer_ms: u32, seed: Option<&[u8]>) -> Self {
        let mut hold = Self {
            mode,
            max_bytes: max_buffer_ms as usize * BYTES_PER_MS,
            pcm: Vec::new(),
            dropped_bytes: 0,
            replaces_utterance_id: None,
            first_closed: false,
            finalize_seen: false,
            ever_had_audio: false,
        };
        if let Some(seed) = seed {
            hold.append(seed);
        }
        hold
    }

    /// Appends PCM (mode rules and the bound apply).
    pub fn push(&mut self, chunk: &[u8]) {
        if self.mode == SttHoldMode::FirstUtterance && self.first_closed {
            self.dropped_bytes += chunk.len() as u64;
            return;
        }
        self.append(chunk);
    }

    fn append(&mut self, chunk: &[u8]) {
        self.ever_had_audio = true;
        self.pcm.extend_from_slice(chunk);
        if self.pcm.len() > self.max_bytes {
            // Drop oldest, keep s16 sample alignment.
            let mut excess = self.pcm.len() - self.max_bytes;
            excess += excess % 2;
            let excess = excess.min(self.pcm.len());
            self.pcm.drain(..excess);
            self.dropped_bytes += excess as u64;
        }
    }

    /// Marks the end of an utterance while held.
    pub fn note_finalize(&mut self) {
        self.finalize_seen = true;
        if self.mode == SttHoldMode::FirstUtterance && self.ever_had_audio {
            self.first_closed = true;
        }
    }

    pub fn take_pcm(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pcm)
    }

    pub fn is_empty(&self) -> bool {
        self.pcm.is_empty()
    }

    pub fn buffered_ms(&self) -> u32 {
        pcm_bytes_to_ms(self.pcm.len())
    }

    pub fn dropped_ms(&self) -> u32 {
        (self.dropped_bytes / BYTES_PER_MS as u64) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_oldest_and_counts() {
        let mut hold = SttHold::new(SttHoldMode::BufferReplay, 10, None);
        hold.push(&[1_u8; 320]);
        hold.push(&[2_u8; 320]);
        assert_eq!(hold.buffered_ms(), 10);
        assert_eq!(hold.dropped_ms(), 10);
        let pcm = hold.take_pcm();
        assert!(pcm.iter().all(|b| *b == 2));
    }

    #[test]
    fn first_utterance_drops_after_close() {
        let mut hold = SttHold::new(SttHoldMode::FirstUtterance, 1000, None);
        hold.push(&[1_u8; 64]);
        hold.note_finalize();
        hold.push(&[2_u8; 64]);
        assert_eq!(hold.buffered_ms(), 2);
        assert_eq!(hold.dropped_ms(), 2);
    }

    #[test]
    fn caps_and_defaults() {
        assert_eq!(resolved_hold_max_buffer_ms(None), 45_000);
        assert_eq!(resolved_hold_max_buffer_ms(Some(999_999)), 120_000);
        assert_eq!(
            resolved_replay_max_age_ms(&ReplayConfig { max_age_ms: None }),
            10_000
        );
        assert_eq!(
            resolved_replay_max_age_ms(&ReplayConfig {
                max_age_ms: Some(500_000)
            }),
            120_000
        );
    }
}
