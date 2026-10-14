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

/// How long a short voiced burst that ended without VAD `SpeechStart` is kept for the next utterance (a quiet first word followed by a pause of up to 3 s).
pub const SHORT_BURST_RETAIN_MS: u32 = 3000;
/// Cap on the length of the retained audio.
pub const SHORT_BURST_MAX_AUDIO_MS: u32 = 1500;
/// Silence inserted between a retained burst and the pre-roll ring when they are not contiguous.
pub const SHORT_BURST_SEPARATOR_MS: u32 = 200;

/// Keeps short voiced bursts (below `min_speech_duration_ms`, so no VAD `SpeechStart`) for
/// [`SHORT_BURST_RETAIN_MS`], so the part that already fell out of the pre-roll ring can be
/// prepended when the next utterance opens a new STT stream. Mono s16le 16 kHz bytes.
#[derive(Default)]
pub struct ShortBurstRetainer {
    /// Voiced pending frames since the last inactive frame.
    current: Vec<u8>,
    /// Bursts that ended (gaps between them filled with zeros).
    retained: Vec<u8>,
    /// Media ms after the end of `retained` (every frame pushed after it).
    since_retained_end_ms: u32,
}

impl ShortBurstRetainer {
    pub fn new() -> Self {
        Self::default()
    }

    fn cap_bytes() -> usize {
        mono_s16le_bytes_for_duration_ms(SHORT_BURST_MAX_AUDIO_MS)
    }

    /// Feed one frame that went to the pre-roll ring while the gate was closed.
    pub fn on_frame(&mut self, mono: &[u8], voiced_pending: bool, duration_ms: u32) {
        if voiced_pending {
            self.current.extend_from_slice(mono);
            if !self.retained.is_empty() {
                self.since_retained_end_ms = self.since_retained_end_ms.saturating_add(duration_ms);
            }
            return;
        }
        if !self.current.is_empty() {
            if !self.retained.is_empty() {
                // `since_retained_end_ms` also counts the voiced frames of `current`, which are
                // appended below, so only the remainder is the gap.
                let current_ms = mono_duration_ms_from_bytes(self.current.len());
                let gap = mono_s16le_bytes_for_duration_ms(
                    self.since_retained_end_ms.saturating_sub(current_ms),
                );
                self.retained.resize(self.retained.len() + gap, 0);
            }
            let current = std::mem::take(&mut self.current);
            self.retained.extend_from_slice(&current);
            let cap = Self::cap_bytes();
            if self.retained.len() > cap {
                let drop_len = (self.retained.len() - cap + 1) & !1;
                self.retained.drain(..drop_len);
            }
            self.since_retained_end_ms = 0;
        }
        if !self.retained.is_empty() {
            self.since_retained_end_ms = self.since_retained_end_ms.saturating_add(duration_ms);
            if self.since_retained_end_ms > SHORT_BURST_RETAIN_MS {
                self.retained.clear();
                self.since_retained_end_ms = 0;
            }
        }
    }

    /// At a SpeechStart that opens a new stream: the part of the retained burst that is no
    /// longer in the ring (head), and whether it is contiguous with the ring content.
    pub fn take_for_new_stream(&mut self, ring_capacity_ms: u32) -> Option<(Bytes, bool)> {
        let result = if self.retained.is_empty() {
            None
        } else {
            let retained_ms = mono_duration_ms_from_bytes(self.retained.len());
            let age_ms = self.since_retained_end_ms;
            let in_ring_ms = ring_capacity_ms.saturating_sub(age_ms).min(retained_ms);
            let out_ms = retained_ms - in_ring_ms;
            if out_ms == 0 {
                None
            } else {
                let end = mono_s16le_bytes_for_duration_ms(out_ms).min(self.retained.len());
                Some((
                    Bytes::copy_from_slice(&self.retained[..end]),
                    in_ring_ms > 0,
                ))
            }
        };
        self.clear();
        result
    }

    pub fn clear(&mut self) {
        self.current.clear();
        self.retained.clear();
        self.since_retained_end_ms = 0;
    }
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
        let config = crate::config::VadConfig {
            min_speech_duration_ms: 250,
            speech_pad_ms: 30,
            ..crate::config::VadConfig::default()
        };
        let ring = SttPreRollBuffer::from_vad_config(&config);
        assert_eq!(ring.capacity_ms(), 280);
    }

    #[test]
    fn capacity_prefers_speech_pad_lead_in() {
        let config = crate::config::VadConfig {
            min_speech_duration_ms: 80,
            speech_pad_ms: 400,
            ..crate::config::VadConfig::default()
        };
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

    fn feed(r: &mut ShortBurstRetainer, voiced: bool, frames: u32, fill: u8) {
        for _ in 0..frames {
            r.on_frame(&vec![fill; 640], voiced, 20);
        }
    }

    #[test]
    fn burst_then_silence_within_window_is_kept() {
        let mut r = ShortBurstRetainer::new();
        feed(&mut r, true, 8, 7);
        feed(&mut r, false, 50, 0);
        assert_eq!(r.retained.len(), 8 * 640);
        assert_eq!(r.since_retained_end_ms, 1000);
    }

    #[test]
    fn burst_older_than_window_is_cleared() {
        let mut r = ShortBurstRetainer::new();
        feed(&mut r, true, 8, 7);
        feed(&mut r, false, 151, 0);
        assert!(r.retained.is_empty());
        assert!(r.take_for_new_stream(700).is_none());
    }

    #[test]
    fn burst_fully_in_ring_returns_none() {
        let mut r = ShortBurstRetainer::new();
        feed(&mut r, true, 8, 7);
        feed(&mut r, false, 15, 0);
        assert!(r.take_for_new_stream(700).is_none());
        assert!(r.retained.is_empty());
    }

    #[test]
    fn partial_overlap_returns_out_of_ring_head_contiguous() {
        let mut r = ShortBurstRetainer::new();
        feed(&mut r, true, 8, 7); // 160 ms
        feed(&mut r, false, 30, 0); // age 600 ms, ring 700 -> 100 ms of burst in ring
        let (head, contiguous) = r.take_for_new_stream(700).unwrap();
        assert!(contiguous);
        assert_eq!(head.len(), mono_s16le_bytes_for_duration_ms(60));
        assert!(head.iter().all(|&b| b == 7));
    }

    #[test]
    fn no_overlap_returns_whole_burst_separated() {
        let mut r = ShortBurstRetainer::new();
        feed(&mut r, true, 8, 7);
        feed(&mut r, false, 70, 0); // age 1400 > ring 700
        let (head, contiguous) = r.take_for_new_stream(700).unwrap();
        assert!(!contiguous);
        assert_eq!(head.len(), 8 * 640);
    }

    #[test]
    fn two_bursts_are_joined_with_zero_gap() {
        let mut r = ShortBurstRetainer::new();
        feed(&mut r, true, 3, 7);
        feed(&mut r, false, 10, 0); // 200 ms gap
        feed(&mut r, true, 3, 9);
        feed(&mut r, false, 1, 0);
        assert_eq!(r.retained.len(), (3 + 10 + 3) * 640);
        assert!(r.retained[..3 * 640].iter().all(|&b| b == 7));
        assert!(r.retained[3 * 640..13 * 640].iter().all(|&b| b == 0));
        assert!(r.retained[13 * 640..].iter().all(|&b| b == 9));
    }

    #[test]
    fn retained_is_capped_at_window() {
        let mut r = ShortBurstRetainer::new();
        feed(&mut r, true, 60, 5); // 1200 ms
        feed(&mut r, false, 1, 0);
        feed(&mut r, true, 40, 6); // 800 ms
        feed(&mut r, false, 1, 0);
        assert_eq!(r.retained.len(), mono_s16le_bytes_for_duration_ms(1500));
        assert!(r.retained.ends_with(&vec![6u8; 40 * 640]));
    }
}
