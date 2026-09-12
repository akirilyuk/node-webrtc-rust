//! Ring buffer of recent mono STT PCM — prepended when VAD speech starts.
//!
//! Used when [`crate::config::VadConfig::gate_stt`] is true: while the STT pipeline is active,
//! every inbound frame that is not pushed directly to STT is retained (continuous lookback),
//! including silence and sub-threshold onset before VAD `SpeechStart`. Flushed at `SpeechStart`
//! so soft syllable onsets are not clipped. Capacity derives from `speech_pad_ms` +
//! `min_speech_duration_ms`.

use std::collections::VecDeque;

use bytes::Bytes;

use crate::config::VadConfig;
use crate::pcm::STT_PCM_SAMPLE_RATE;

/// Rolling buffer of mono s16le PCM at 16 kHz for STT pre-roll.
pub struct SttPreRollBuffer {
    max_bytes: usize,
    data: VecDeque<u8>,
}

impl SttPreRollBuffer {
    pub fn from_vad_config(config: &VadConfig) -> Self {
        Self::new(stt_pre_roll_capacity_ms(config))
    }

    pub fn new(capacity_ms: u32) -> Self {
        let max_bytes = mono_s16le_bytes_for_duration_ms(capacity_ms.max(1));
        Self {
            max_bytes,
            data: VecDeque::with_capacity(max_bytes.min(65_536)),
        }
    }

    pub fn capacity_ms(&self) -> u32 {
        mono_duration_ms_from_bytes(self.max_bytes)
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Append mono PCM and drop the oldest samples when over capacity.
    pub fn push(&mut self, mono_bytes: &[u8]) {
        if mono_bytes.is_empty() {
            return;
        }
        self.data.extend(mono_bytes);
        while self.data.len() > self.max_bytes {
            let overflow = self.data.len() - self.max_bytes;
            self.data.drain(0..overflow);
        }
    }

    /// Take all buffered mono PCM (oldest first).
    pub fn drain(&mut self) -> Bytes {
        if self.data.is_empty() {
            return Bytes::new();
        }
        let out: Vec<u8> = self.data.drain(..).collect();
        Bytes::from(out)
    }

    /// Drain buffered mono PCM, left-padding with digital silence when the ring is not full.
    ///
    /// Streaming recognizers (e.g. Zipformer) need lead-in context before the first spoken
    /// sample. A cold inbound path (DTX sender, push-to-talk, loopback harness) may deliver no
    /// frames before the talker's onset, so the ring holds only the TTS/speech onset at
    /// VAD `SpeechStart`. Padding to [`Self::capacity_ms`] makes that flush byte-identical in
    /// shape to a warm path where silence filled the ring first: `[silence pad][buffered onset]`.
    ///
    /// Returns empty when the ring is empty — that means the STT stream was already receiving
    /// frames directly and padding would inject silence mid-stream. Callers opening a **new**
    /// recognizer stream must only use this when starting a fresh stream, not when continuing
    /// after a brief-gap `SpeechStart` on an already-open stream.
    pub fn drain_padded_to_capacity(&mut self) -> Bytes {
        if self.data.is_empty() {
            return Bytes::new();
        }
        let buffered_len = self.data.len();
        if buffered_len >= self.max_bytes {
            return self.drain();
        }
        let pad_len = self.max_bytes - buffered_len;
        let even_pad_len = pad_len & !1;
        let mut out = Vec::with_capacity(self.max_bytes);
        out.resize(even_pad_len, 0);
        out.extend(self.data.drain(..));
        Bytes::from(out)
    }

    /// Milliseconds of silence padding [`drain_padded_to_capacity`] would prepend for the
    /// current buffered length (0 when the ring is empty or already at capacity).
    pub fn pad_ms_for_len(&self) -> u32 {
        if self.data.is_empty() || self.data.len() >= self.max_bytes {
            return 0;
        }
        mono_duration_ms_from_bytes(self.max_bytes - self.data.len())
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }
}

/// How much mono audio to retain before VAD `SpeechStart`.
pub fn stt_pre_roll_capacity_ms(config: &VadConfig) -> u32 {
    config
        .speech_pad_ms
        .saturating_add(config.min_speech_duration_ms)
}

fn mono_s16le_bytes_for_duration_ms(duration_ms: u32) -> usize {
    (STT_PCM_SAMPLE_RATE as usize * duration_ms as usize / 1000).saturating_mul(2)
}

fn mono_duration_ms_from_bytes(byte_len: usize) -> u32 {
    ((byte_len / 2) as u64 * 1000 / STT_PCM_SAMPLE_RATE as u64).max(1) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_drops_oldest_samples_when_over_capacity() {
        let mut ring = SttPreRollBuffer::new(20);
        let frame = vec![0_u8; 640];
        ring.push(&frame);
        ring.push(&frame);
        assert!(ring.len() <= ring.max_bytes);
        let drained = ring.drain();
        assert!(!drained.is_empty());
        assert!(ring.is_empty());
    }

    #[test]
    fn capacity_from_vad_config_covers_speech_window() {
        let mut config = crate::config::VadConfig::default();
        config.min_speech_duration_ms = 250;
        config.speech_pad_ms = 30;
        let ring = SttPreRollBuffer::from_vad_config(&config);
        assert_eq!(ring.capacity_ms(), 280);
    }

    #[test]
    fn capacity_prefers_speech_pad_lead_in() {
        let mut config = crate::config::VadConfig::default();
        config.min_speech_duration_ms = 80;
        config.speech_pad_ms = 400;
        let ring = SttPreRollBuffer::from_vad_config(&config);
        assert_eq!(ring.capacity_ms(), 480);
    }

    #[test]
    fn drain_padded_short_ring_prepends_silence_to_capacity() {
        let mut ring = SttPreRollBuffer::new(700);
        let frame_20ms = vec![0xAB_u8; 640]; // 20 ms mono @ 16 kHz
        for _ in 0..12 {
            ring.push(&frame_20ms);
        }
        assert_eq!(ring.len(), 12 * 640);
        let padded = ring.drain_padded_to_capacity();
        let capacity_bytes = mono_s16le_bytes_for_duration_ms(700);
        assert_eq!(padded.len(), capacity_bytes);
        let pad_len = capacity_bytes - 12 * 640;
        assert!(pad_len > 0);
        assert!(padded[..pad_len].iter().all(|&b| b == 0));
        let mut expected_tail = Vec::with_capacity(12 * 640);
        for _ in 0..12 {
            expected_tail.extend_from_slice(&frame_20ms);
        }
        assert_eq!(&padded[pad_len..], expected_tail.as_slice());
        assert!(ring.is_empty());
    }

    #[test]
    fn drain_padded_empty_ring_returns_empty() {
        let mut ring = SttPreRollBuffer::new(700);
        assert!(ring.drain_padded_to_capacity().is_empty());
    }

    #[test]
    fn drain_padded_full_ring_matches_plain_drain() {
        let mut ring = SttPreRollBuffer::new(700);
        let frame = vec![0xCD_u8; 640];
        for _ in 0..40 {
            ring.push(&frame);
        }
        assert_eq!(ring.len(), mono_s16le_bytes_for_duration_ms(700));
        let plain = {
            let mut r = SttPreRollBuffer::new(700);
            for _ in 0..40 {
                r.push(&frame);
            }
            r.drain()
        };
        let padded = ring.drain_padded_to_capacity();
        assert_eq!(padded, plain);
    }
}
