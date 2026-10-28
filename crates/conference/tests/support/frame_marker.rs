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
    /// Marker distance walked from the first to the last recognised frame: the sum of the
    /// unwrapped forward steps between advancing markers. A stream of N source frames has a
    /// span of at most N - 1, so a replay that the merge budget lets through as a long forward
    /// step shows up as a span about 40 too long.
    pub span: u64,
}

/// Merge budget for [`analyze_with`] (mirrors `AnalyseOptions` in the TypeScript twin).
#[derive(Debug, Default, Clone, Copy)]
pub struct AnalyzeOptions {
    /// Output frames the mixer reports as merged (`compressed + overflow_compressed +
    /// listener_catchup_compressed + listener_skipped`). The sequence is unwrapped mod 40,
    /// which is unambiguous only for a forward step of at most 20; a step above 20 counts as
    /// forward when the indices it skips beyond its unknown frames are paid for by merges not
    /// used yet (at most `per frame` each). Anything else above 20 is `reordered`.
    pub merged_frames: u64,
    /// How many of `merged_frames` are listener catch-up merges. A catch-up frame cross-fades
    /// two ticks that may each be a queue-compressed pair, so when both kinds occur one frame
    /// can absorb up to [`MAX_STACKED_MERGE_INDICES`] indices instead of one.
    pub listener_catchup_frames: u64,
}

/// Indices one output frame can absorb when listener catch-up merges two compressed ticks.
pub const MAX_STACKED_MERGE_INDICES: u64 = 3;

/// [`analyze_with`] without a merge budget: every step above 20 is `reordered`.
pub fn analyze(frames: &[Detected]) -> SequenceReport {
    analyze_with(frames, AnalyzeOptions::default())
}

/// Walks `frames` and reports gaps/duplicates after the first marker. An ambiguous or
/// interior-silent frame stands in for one unknown frame, so it explains one missing index.
/// Steps above half the period are resolved with the merge budget in `opts`.
pub fn analyze_with(frames: &[Detected], opts: AnalyzeOptions) -> SequenceReport {
    let mut report = SequenceReport::default();
    let mut merges_left = opts.merged_frames;
    let catchup = opts.listener_catchup_frames.min(merges_left);
    let per_frame = if catchup > 0 && merges_left > catchup {
        MAX_STACKED_MERGE_INDICES
    } else {
        1
    };
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
                }
                // Indices this step skips that no unknown frame stands in for.
                let unexplained = (delta as u64 - 1).saturating_sub(unknown);
                let long_forward = delta > MARKER_PERIOD / 2
                    && unexplained <= merges_left.min(per_frame * (unknown + 1));
                if delta > MARKER_PERIOD / 2 && !long_forward {
                    report.reordered += 1;
                } else {
                    if long_forward {
                        merges_left -= unexplained;
                    }
                    report.gaps += unexplained;
                    report.span += delta as u64;
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
        assert_eq!(r.span, 1 + 1 + 3 + 3);
    }

    /// 0..=9 heard, 5 merged frames (ambiguous), then the marker 24 on (9 + 24 = 33): six
    /// frames carried 23 indices, as listener catch-up over compressed ticks does.
    fn merged_long_step() -> Vec<Detected> {
        use Detected::*;
        let mut seq: Vec<Detected> = (0..10).map(Index).collect();
        seq.extend(std::iter::repeat_n(Ambiguous, 5));
        seq.extend((33..40).map(Index));
        seq
    }

    #[test]
    fn a_long_step_paid_for_by_counted_merges_is_forward() {
        let seq = merged_long_step();
        // No budget: the 24-index step reads as backwards (and `prev` does not resync, so
        // every later marker of the run does too).
        assert!(analyze(&seq).reordered > 0);
        let r = analyze_with(
            &seq,
            AnalyzeOptions {
                merged_frames: 18,
                listener_catchup_frames: 6,
            },
        );
        assert_eq!(r.reordered, 0, "{r:?}");
        assert_eq!(r.gaps, 18, "{r:?}");
        assert_eq!(r.span, 9 + 24 + 6, "{r:?}");
        // Too few merges to pay for the skipped indices: still reordered.
        let short = analyze_with(
            &seq,
            AnalyzeOptions {
                merged_frames: 17,
                listener_catchup_frames: 6,
            },
        );
        assert!(short.reordered > 0, "{short:?}");
        // Catch-up merges only: one index per frame, so six frames explain 6 of 18.
        let single = analyze_with(
            &seq,
            AnalyzeOptions {
                merged_frames: 40,
                listener_catchup_frames: 40,
            },
        );
        assert!(single.reordered > 0, "{single:?}");
    }

    #[test]
    fn an_adjacent_swap_is_reordered_whatever_the_merge_budget() {
        use Detected::*;
        let seq = [Index(0), Index(1), Index(3), Index(2), Index(4), Index(5)];
        let r = analyze_with(
            &seq,
            AnalyzeOptions {
                merged_frames: 100,
                listener_catchup_frames: 50,
            },
        );
        assert!(r.reordered >= 1, "{r:?}");
    }
}
