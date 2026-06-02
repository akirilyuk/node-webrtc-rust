//! Ring buffer of recent mono STT PCM — prepended when VAD speech starts.
//!
//! Used when [`crate::config::VadConfig::gate_stt`] is true: captures audio during VAD
//! pending/speech frames (excluding silence) and flushes at `SpeechStart` so the first
//! syllable is not clipped. Capacity derives from `speech_pad_ms` + `min_speech_duration_ms`.

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
}
