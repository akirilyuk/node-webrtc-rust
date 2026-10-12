//! Frame-index marker for audio continuity tests.
//!
//! Every 20 ms source frame (960 samples per channel at 48 kHz, stereo L = R) carries one
//! sine at `f(i) = 400 + 50 * (i mod 40)` Hz, amplitude 0.3 full scale, phase-continuous
//! across frames. 50 Hz spacing equals the 20 ms Goertzel bin width, so the 40 candidates
//! are orthogonal on a clean frame. The detector returns the index when the strongest
//! candidate is at least 6 dB above the runner-up, otherwise `Ambiguous` (a cross-faded
//! merge, a codec transition or a concealment). Sequence checks unwrap indices mod 40.
//!
//! The TypeScript twin lives in `packages/helpers/tests/support/frame-marker.ts`; keep the
//! constants identical.

#![allow(dead_code)]

use bytes::Bytes;
use node_webrtc_rust_mixer::{Frame, FRAME_BYTES, SAMPLES_PER_CHANNEL, SAMPLE_RATE};

/// Number of distinct marker indices (index wraps mod this).
pub const MARKER_PERIOD: usize = 40;
const BASE_HZ: f64 = 400.0;
const STEP_HZ: f64 = 50.0;
const AMPLITUDE: f64 = 0.3;
/// Required margin of the strongest candidate over the runner-up.
const MARGIN_DB: f64 = 6.0;
/// Frames with RMS (full scale = 1.0) below this are `Silent`.
const SILENCE_RMS: f64 = 0.01;

/// Marker frequency for frame index `i`.
pub fn marker_hz(i: u64) -> f64 {
    BASE_HZ + STEP_HZ * (i % MARKER_PERIOD as u64) as f64
}

/// Generates phase-continuous marker frames.
#[derive(Default)]
pub struct MarkerSource {
    phase: f64,
}

impl MarkerSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns frame `i` (call with consecutive indices to stay phase-continuous).
    pub fn frame(&mut self, i: u64) -> Frame {
        let step = 2.0 * std::f64::consts::PI * marker_hz(i) / f64::from(SAMPLE_RATE);
        let mut pcm = vec![0u8; FRAME_BYTES];
        for n in 0..SAMPLES_PER_CHANNEL {
            let v = (AMPLITUDE * 32_767.0 * self.phase.sin()).round() as i16;
            self.phase = (self.phase + step) % (2.0 * std::f64::consts::PI);
            let b = v.to_le_bytes();
            let base = n * 4;
            pcm[base..base + 2].copy_from_slice(&b);
            pcm[base + 2..base + 4].copy_from_slice(&b);
        }
        Frame::new(Bytes::from(pcm), None)
    }
}

/// Result of classifying one received 20 ms frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detected {
    Silent,
    Ambiguous,
    Index(usize),
}

fn goertzel_power(samples: &[f64], hz: f64) -> f64 {
    let n = samples.len() as f64;
    let k = (0.5 + n * hz / f64::from(SAMPLE_RATE)).floor();
    let omega = 2.0 * std::f64::consts::PI * k / n;
    let coeff = 2.0 * omega.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in samples {
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

/// Classifies one received frame (interleaved stereo s16le; left channel is analysed).
pub fn detect_frame(pcm: &[u8]) -> Detected {
    if pcm.len() != FRAME_BYTES {
        return Detected::Silent;
    }
    let left: Vec<f64> = pcm
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f64::from(i16::from_le_bytes([c[0], c[1]])) / 32_768.0)
        .collect();
    let rms = (left.iter().map(|x| x * x).sum::<f64>() / left.len() as f64).sqrt();
    if rms < SILENCE_RMS {
        return Detected::Silent;
    }
    let mut powers: Vec<(usize, f64)> = (0..MARKER_PERIOD)
        .map(|k| (k, goertzel_power(&left, marker_hz(k as u64))))
        .collect();
    powers.sort_by(|a, b| b.1.total_cmp(&a.1));
    let (best, runner_up) = (powers[0], powers[1]);
    if runner_up.1 <= 0.0 || 10.0 * (best.1 / runner_up.1).log10() >= MARGIN_DB {
        Detected::Index(best.0)
    } else {
        Detected::Ambiguous
    }
}

/// Continuity summary of a received sequence.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SequenceReport {
    /// Distinct marker frames heard (index detections that advanced the sequence).
    pub played: u64,
    /// Marker frames missing between heard frames (not explained by an unknown frame).
    pub gaps: u64,
    /// Index repeated back-to-back (a replayed or concealed frame) before a later marker.
    pub duplicates: u64,
    /// Repeats after the last advancing marker (the end-of-stream concealment tail).
    pub trailing_duplicates: u64,
    /// Indices that went backwards (reordering).
    pub reordered: u64,
    /// Ambiguous frames after the first marker.
    pub ambiguous: u64,
    /// Silent frames between the first and last marker.
    pub interior_silence: u64,
}

/// Walks `frames` and reports gaps/duplicates after the first marker. An ambiguous or
/// interior-silent frame stands in for one unknown frame, so it explains one missing index.
pub fn analyze(frames: &[Detected]) -> SequenceReport {
    let mut report = SequenceReport::default();
    let last_marker = frames.iter().rposition(|d| matches!(d, Detected::Index(_)));
    let Some(last_marker) = last_marker else {
        return report;
    };
    let mut prev: Option<usize> = None;
    let mut unknown = 0u64;
    let mut pending_dups = 0u64;
    for (pos, d) in frames.iter().enumerate().take(last_marker + 1) {
        match (*d, prev) {
            (Detected::Index(i), None) => {
                prev = Some(i);
                report.played += 1;
            }
            (Detected::Index(i), Some(p)) => {
                let delta = (i + MARKER_PERIOD - p) % MARKER_PERIOD;
                if delta == 0 {
                    pending_dups += 1;
                    continue;
                } else if delta > MARKER_PERIOD / 2 {
                    report.reordered += 1;
                } else {
                    let missing = delta as u64 - 1;
                    report.gaps += missing.saturating_sub(unknown);
                    report.played += 1;
                    prev = Some(i);
                    report.duplicates += pending_dups;
                    pending_dups = 0;
                }
                unknown = 0;
            }
            (Detected::Ambiguous, Some(_)) => {
                report.ambiguous += 1;
                unknown += 1;
            }
            (Detected::Silent, Some(_)) => {
                report.interior_silence += 1;
                unknown += 1;
            }
            _ => {}
        }
        let _ = pos;
    }
    report.trailing_duplicates = pending_dups;
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_frames_are_detected_with_their_own_index() {
        let mut src = MarkerSource::new();
        for i in 0..80u64 {
            let f = src.frame(i);
            assert_eq!(
                detect_frame(&f.pcm),
                Detected::Index((i % 40) as usize),
                "i={i}"
            );
        }
    }

    #[test]
    fn silence_is_silent_and_a_two_tone_mix_is_ambiguous() {
        assert_eq!(detect_frame(&[0u8; FRAME_BYTES]), Detected::Silent);
        let (mut a, mut b) = (MarkerSource::new(), MarkerSource::new());
        let fa = a.frame(3);
        let fb = b.frame(17);
        let mixed: Vec<u8> = fa
            .pcm
            .as_chunks::<2>()
            .0
            .iter()
            .zip(fb.pcm.as_chunks::<2>().0.iter())
            .flat_map(|(x, y)| {
                let s = i16::from_le_bytes([x[0], x[1]]) / 2 + i16::from_le_bytes([y[0], y[1]]) / 2;
                s.to_le_bytes()
            })
            .collect();
        assert_eq!(detect_frame(&mixed), Detected::Ambiguous);
    }

    #[test]
    fn analyze_counts_gaps_duplicates_and_unwraps_mod_40() {
        use Detected::*;
        let seq = [
            Silent,
            Index(38),
            Index(39),
            Index(0),
            Index(0),
            Index(3),
            Ambiguous,
            Index(6),
            Silent,
        ];
        let r = analyze(&seq);
        assert_eq!(r.played, 5);
        assert_eq!(r.duplicates, 1);
        assert_eq!(r.trailing_duplicates, 0);
        // 1,2 missing between 0 and 3 (2); 4,5 missing between 3 and 6 minus one unknown (1).
        assert_eq!(r.gaps, 3);
        assert_eq!(r.ambiguous, 1);
    }
}
