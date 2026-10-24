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

/// Cap on concealed (PLC/FEC) audio kept in the ring, in ms. Concealed frames do not use the
/// received-audio budget, but an unbounded run would grow the ring without limit; at 15 % bursty
/// loss the onset sat 0.7-0.9 s before VAD `SpeechStart`, and 500 ms of concealment covers it.
pub const PRE_ROLL_MAX_CONCEALED_MS: u32 = 500;

/// Rolling buffer of mono s16le PCM at 16 kHz for STT pre-roll.
///
/// The capacity applies to **received** audio. Frames flagged as concealed (synthesized by loss
/// concealment) are kept too, but do not consume that budget (they have their own cap of
/// [`PRE_ROLL_MAX_CONCEALED_MS`]), so loss does not push the onset out of the ring.
pub struct SttPreRollBuffer {
    max_bytes: usize,
    max_concealed_bytes: usize,
    data: VecDeque<u8>,
    /// (byte length, concealed) per pushed segment, oldest first; lengths sum to `data.len()`.
    segments: VecDeque<(usize, bool)>,
    received_bytes: usize,
    concealed_bytes: usize,
}

impl SttPreRollBuffer {
    pub fn from_vad_config(config: &VadConfig) -> Self {
        Self::new(stt_pre_roll_capacity_ms(config))
    }

    pub fn new(capacity_ms: u32) -> Self {
        let max_bytes = mono_s16le_bytes_for_duration_ms(capacity_ms.max(1));
        Self {
            max_bytes,
            max_concealed_bytes: mono_s16le_bytes_for_duration_ms(PRE_ROLL_MAX_CONCEALED_MS),
            data: VecDeque::with_capacity(max_bytes.min(65_536)),
            segments: VecDeque::new(),
            received_bytes: 0,
            concealed_bytes: 0,
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

    /// Append received mono PCM and drop the oldest samples when over capacity.
    pub fn push(&mut self, mono_bytes: &[u8]) {
        self.push_frame(mono_bytes, false);
    }

    /// Append mono PCM flagged as received (`concealed = false`) or concealed, then trim:
    /// received bytes over the capacity are dropped from the front, concealed bytes over
    /// [`PRE_ROLL_MAX_CONCEALED_MS`] are dropped oldest-concealed-first.
    pub fn push_frame(&mut self, mono_bytes: &[u8], concealed: bool) {
        if mono_bytes.is_empty() {
            return;
        }
        self.data.extend(mono_bytes);
        self.segments.push_back((mono_bytes.len(), concealed));
        if concealed {
            self.concealed_bytes += mono_bytes.len();
        } else {
            self.received_bytes += mono_bytes.len();
        }
        // Received overflow: drop from the front (everything there is older).
        while self.received_bytes > self.max_bytes {
            let over = self.received_bytes - self.max_bytes;
            self.drop_from_segment(0, over);
        }
        // Concealed overflow: drop the oldest concealed bytes, wherever they sit, so received
        // audio is never sacrificed for concealment.
        while self.concealed_bytes > self.max_concealed_bytes {
            let over = self.concealed_bytes - self.max_concealed_bytes;
            let Some(idx) = self.segments.iter().position(|(_, c)| *c) else {
                break;
            };
            self.drop_from_segment(idx, over);
        }
    }

    /// Drop up to `want` bytes (even count, at most the segment) from the start of segment
    /// `idx`, keeping the segment list and counters exact.
    fn drop_from_segment(&mut self, idx: usize, want: usize) {
        let (seg_len, seg_concealed) = self.segments[idx];
        let drop_len = if want >= seg_len {
            seg_len
        } else {
            // Keep s16 samples aligned: drop an even number of bytes, at least 2.
            ((want + 1) & !1).max(2).min(seg_len)
        };
        let offset: usize = self.segments.iter().take(idx).map(|(l, _)| *l).sum();
        self.data.drain(offset..offset + drop_len);
        if seg_concealed {
            self.concealed_bytes -= drop_len;
        } else {
            self.received_bytes -= drop_len;
        }
        if drop_len == seg_len {
            self.segments.remove(idx);
        } else {
            self.segments[idx].0 -= drop_len;
        }
    }

    /// Take all buffered mono PCM (oldest first).
    pub fn drain(&mut self) -> Bytes {
        if self.data.is_empty() {
            return Bytes::new();
        }
        self.reset_counters();
        let out: Vec<u8> = self.data.drain(..).collect();
        Bytes::from(out)
    }

    fn reset_counters(&mut self) {
        self.segments.clear();
        self.received_bytes = 0;
        self.concealed_bytes = 0;
    }

    /// Drain buffered mono PCM, left-padding with digital silence when the ring is not full.
    ///
    /// Streaming recognizers (e.g. Zipformer) need lead-in context before the first spoken
    /// sample. A cold inbound path (DTX sender, push-to-talk, loopback harness) may deliver no
    /// frames before the talker's onset, so the ring holds only the TTS/speech onset at
    /// VAD `SpeechStart`. Padding to [`Self::capacity_ms`] (counting received bytes only, so
    /// concealed frames add real time on top of the pad) makes that flush byte-identical in
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
        if self.received_bytes >= self.max_bytes {
            return self.drain();
        }
        let pad_len = self.max_bytes - self.received_bytes;
        let even_pad_len = pad_len & !1;
        let mut out = Vec::with_capacity(self.max_bytes + self.data.len());
        out.resize(even_pad_len, 0);
        out.extend(self.data.drain(..));
        self.reset_counters();
        Bytes::from(out)
    }

    /// Milliseconds of silence padding [`drain_padded_to_capacity`] would prepend for the
    /// current received-byte count (0 when the ring is empty or received bytes are at capacity).
    pub fn pad_ms_for_len(&self) -> u32 {
        if self.data.is_empty() || self.received_bytes >= self.max_bytes {
            return 0;
        }
        mono_duration_ms_from_bytes(self.max_bytes - self.received_bytes)
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.reset_counters();
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

    const FRAME: usize = 640;

    fn frame(fill: u8) -> Vec<u8> {
        vec![fill; FRAME]
    }

    #[test]
    fn concealed_frames_do_not_use_the_received_budget() {
        let mut r = SttPreRollBuffer::new(700);
        let mut pushed = 0;
        let mut received = 0;
        let mut concealed = 0;
        while received < 35 || concealed < 7 {
            let conceal = pushed % 6 == 5 && concealed < 7;
            if conceal {
                r.push_frame(&frame(2), true);
                concealed += 1;
            } else if received < 35 {
                r.push_frame(&frame(1), false);
                received += 1;
            }
            pushed += 1;
        }
        assert_eq!(r.len(), 42 * FRAME);
    }

    #[test]
    fn concealed_extension_is_capped() {
        let mut r = SttPreRollBuffer::new(700);
        let (mut rec, mut con) = (0u32, 0u32);
        for i in 0..75u32 {
            // 35 received + 40 concealed, interleaved roughly evenly.
            let conceal = (i % 2 == 1 && con < 40) || rec >= 35;
            if conceal {
                r.push_frame(&frame(2), true);
                con += 1;
            } else {
                r.push_frame(&frame(1), false);
                rec += 1;
            }
        }
        assert_eq!((rec, con), (35, 40));
        let out = r.drain();
        let ones = out.iter().filter(|b| **b == 1).count();
        let twos = out.iter().filter(|b| **b == 2).count();
        assert_eq!(ones, 700 * 32);
        assert!(twos <= 500 * 32, "concealed bytes {twos}");
        assert_eq!(*out.last().unwrap(), 2, "newest frame is last");
    }

    #[test]
    fn unflagged_push_is_unchanged() {
        let mut r = SttPreRollBuffer::new(700);
        let max = 700 * 32;
        let mut reference: VecDeque<u8> = VecDeque::new();
        for i in 0..100usize {
            let chunk: Vec<u8> = (0..FRAME).map(|j| ((i * 7 + j) % 251) as u8).collect();
            r.push(&chunk);
            reference.extend(&chunk);
            while reference.len() > max {
                let o = reference.len() - max;
                reference.drain(0..o);
            }
        }
        let want: Vec<u8> = reference.into_iter().collect();
        assert_eq!(r.drain().to_vec(), want);
    }

    #[test]
    fn pad_counts_received_bytes_only() {
        let mut r = SttPreRollBuffer::new(700);
        for _ in 0..15 {
            r.push_frame(&frame(1), false);
        }
        for _ in 0..5 {
            r.push_frame(&frame(2), true);
        }
        assert_eq!(r.pad_ms_for_len(), 400);
        let out = r.drain_padded_to_capacity();
        assert_eq!(out.len(), (700 + 100) * 32);
        assert!(out[..400 * 32].iter().all(|b| *b == 0));
        assert_eq!(out[400 * 32], 1);
    }

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
