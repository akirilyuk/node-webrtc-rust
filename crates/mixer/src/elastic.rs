//! Elastic per-source FIFO (adaptive jitter buffer) for the mix graph.
//!
//! A source pushes 20 ms frames whenever they arrive (possibly bunched or late); the
//! mix graph pops exactly one frame per 20 ms media tick. Between the two the queue
//! stretches and shrinks:
//!
//! - **Prefill:** at start-up (and after a long underrun) nothing plays until the queue
//!   holds `target` frames, or the oldest frame has waited `target` ticks.
//! - **Target depth:** `clamp(ceil(jitter / 20 ms) + 1, 2, 8)` frames, where `jitter` is an
//!   EWMA of `|inter-arrival time - 20 ms|` (fast attack, slow decay). Gaps longer than
//!   [`TALKSPURT_GAP`] are pauses: they are left out of the estimate and re-enter prefill.
//! - **Slim (shrink):** when the depth stays above `target + 2` for 10 consecutive ticks,
//!   each tick takes two frames and emits one: a frame below -50 dBFS is dropped,
//!   otherwise the pair is merged with a 10 ms Hann cross-fade (no pitch change).
//! - **Stretch (conceal):** on underrun the last played frame is replayed once with a
//!   fade to -6 dB, then silence; after 3 empty ticks the queue re-enters prefill.
//! - **Overflow:** at capacity (50 frames, 1 s) the two oldest frames are merged instead
//!   of discarding one, and a rate-limited warning is logged (capacity signal).
//!
//! Accounting invariant: `pushed = played + compressed + overflow_compressed + depth`.
//! Concealment frames are not source frames and are counted separately.
//!
//! The queue never reads the clock: callers pass `now` to [`ElasticFrameQueue::push`].

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::frame::{silence_frame, Frame, FRAME_BYTES, FRAME_MS, SAMPLES_PER_CHANNEL};
use crate::otel::{record_mix_frames, MixFrameOutcome};

/// Maximum queued frames per source (1 s).
pub const QUEUE_CAPACITY: usize = 50;
/// Lowest adaptive target depth in frames (40 ms).
pub const MIN_TARGET_FRAMES: usize = 2;
/// Highest adaptive target depth in frames (160 ms).
pub const MAX_TARGET_FRAMES: usize = 8;
/// Depth above `target + SHRINK_MARGIN_FRAMES` (for [`SHRINK_AFTER_TICKS`]) starts slimming.
pub const SHRINK_MARGIN_FRAMES: usize = 2;
/// Consecutive over-target ticks before slimming starts.
pub const SHRINK_AFTER_TICKS: u32 = 10;
/// Empty ticks before the queue re-enters prefill.
pub const REPREFILL_AFTER_EMPTY_TICKS: u32 = 3;
/// Frames quieter than this (dBFS RMS) may be dropped when slimming.
pub const SILENCE_THRESHOLD_DBFS: f64 = -50.0;
/// Cross-fade length in samples per channel (10 ms).
const CROSSFADE_SAMPLES: usize = SAMPLES_PER_CHANNEL / 2;
/// An inter-arrival gap longer than this is a pause (DTX, mute, a turn break), not jitter: it
/// is not fed to the jitter estimate, and the next frame starts a new talk-spurt (prefill).
pub const TALKSPURT_GAP: Duration = Duration::from_millis(200);
/// Minimum spacing between warnings from one queue.
const WARN_INTERVAL: Duration = Duration::from_secs(1);

/// Per-source counters and queue state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceStats {
    /// Source frames pushed.
    pub pushed: u64,
    /// Source frames emitted to the mix (including the surviving frame of a merge).
    pub played: u64,
    /// Source frames absorbed by time-compression while slimming (dropped silence or merged).
    pub compressed: u64,
    /// Concealment frames emitted on underrun.
    pub concealed: u64,
    /// Source frames absorbed by merging at capacity (capacity signal).
    pub overflow_compressed: u64,
    /// Inbound packets for this source that failed to decode (no audio was produced).
    pub decode_errors: u64,
    /// Frames currently queued.
    pub depth: usize,
    /// Current adaptive target depth in frames.
    pub target: usize,
}

/// Adaptive FIFO for one mix source.
#[derive(Debug)]
pub struct ElasticFrameQueue {
    frames: VecDeque<Frame>,
    prefilling: bool,
    prefill_waited: u32,
    over_ticks: u32,
    empty_ticks: u32,
    last_played: Option<Frame>,
    last_arrival: Option<Instant>,
    jitter_ms: f64,
    target: usize,
    pushed: u64,
    played: u64,
    compressed: u64,
    concealed: u64,
    overflow_compressed: u64,
    decode_errors: u64,
    last_warn: Option<Instant>,
    last_decode_warn: Option<Instant>,
    warnings: u64,
}

impl Default for ElasticFrameQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ElasticFrameQueue {
    /// Creates an empty queue in prefill state.
    pub fn new() -> Self {
        Self {
            frames: VecDeque::with_capacity(QUEUE_CAPACITY),
            prefilling: true,
            prefill_waited: 0,
            over_ticks: 0,
            empty_ticks: 0,
            last_played: None,
            last_arrival: None,
            jitter_ms: 0.0,
            target: MIN_TARGET_FRAMES,
            pushed: 0,
            played: 0,
            compressed: 0,
            concealed: 0,
            overflow_compressed: 0,
            decode_errors: 0,
            last_warn: None,
            last_decode_warn: None,
            warnings: 0,
        }
    }

    /// Appends a frame that arrived at `now`. Invalid-length frames are ignored.
    pub fn push(&mut self, frame: Frame, now: Instant) {
        if !frame.is_valid() {
            return;
        }
        self.update_jitter(now);
        self.pushed += 1;

        if self.frames.len() >= QUEUE_CAPACITY {
            if let (Some(a), Some(b)) = (self.frames.pop_front(), self.frames.pop_front()) {
                self.frames.push_front(merge_pair(&a, &b));
                self.overflow_compressed += 1;
                record_mix_frames(MixFrameOutcome::OverflowCompressed, 1);
                self.warn_overflow(now);
            }
        }
        self.frames.push_back(frame);
    }

    /// Counts an undecodable inbound packet and logs it (rate-limited to one per second).
    pub fn note_decode_error(&mut self, reason: &str, now: Instant) {
        self.decode_errors += 1;
        record_mix_frames(MixFrameOutcome::DecodeError, 1);
        let due = match self.last_decode_warn {
            None => true,
            Some(prev) => now.saturating_duration_since(prev) >= WARN_INTERVAL,
        };
        if due {
            self.last_decode_warn = Some(now);
            eprintln!(
                "[node-webrtc-rust mixer] inbound packet could not be decoded ({reason}); decode_errors={}",
                self.decode_errors
            );
        }
    }

    /// Takes the frame for the next 20 ms media tick (silence when prefilling or empty
    /// beyond the one concealment frame).
    pub fn pop(&mut self) -> Frame {
        if self.prefilling {
            if !self.frames.is_empty() {
                self.prefill_waited += 1;
            }
            let ready = self.frames.len() >= self.target
                || (!self.frames.is_empty() && self.prefill_waited as usize >= self.target);
            if !ready {
                return silence_frame();
            }
            self.prefilling = false;
            self.prefill_waited = 0;
        }

        if self.frames.len() > self.target + SHRINK_MARGIN_FRAMES {
            self.over_ticks += 1;
        } else {
            self.over_ticks = 0;
        }

        let Some(first) = self.frames.pop_front() else {
            return self.underrun();
        };
        self.empty_ticks = 0;

        let out = if self.over_ticks >= SHRINK_AFTER_TICKS && !self.frames.is_empty() {
            let second = self.frames.pop_front().unwrap_or_else(silence_frame);
            self.compressed += 1;
            record_mix_frames(MixFrameOutcome::Compressed, 1);
            merge_pair(&first, &second)
        } else {
            first
        };
        self.played += 1;
        record_mix_frames(MixFrameOutcome::Played, 1);
        self.last_played = Some(out.clone());
        out
    }

    /// Counters plus current depth/target.
    pub fn stats(&self) -> SourceStats {
        SourceStats {
            pushed: self.pushed,
            played: self.played,
            compressed: self.compressed,
            concealed: self.concealed,
            overflow_compressed: self.overflow_compressed,
            decode_errors: self.decode_errors,
            depth: self.frames.len(),
            target: self.target,
        }
    }

    /// Frames currently queued.
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    /// Current adaptive target depth in frames.
    pub fn target(&self) -> usize {
        self.target
    }

    /// Number of overflow warnings emitted (rate-limited; for tests).
    pub fn warnings_emitted(&self) -> u64 {
        self.warnings
    }

    fn underrun(&mut self) -> Frame {
        self.empty_ticks += 1;
        let out = if self.empty_ticks == 1 {
            match &self.last_played {
                Some(last) => {
                    self.concealed += 1;
                    record_mix_frames(MixFrameOutcome::Concealed, 1);
                    fade_out(last)
                }
                None => silence_frame(),
            }
        } else {
            silence_frame()
        };
        if self.empty_ticks >= REPREFILL_AFTER_EMPTY_TICKS {
            self.prefilling = true;
            self.prefill_waited = 0;
        }
        out
    }

    fn update_jitter(&mut self, now: Instant) {
        if let Some(prev) = self.last_arrival {
            let gap = now.saturating_duration_since(prev);
            if gap > TALKSPURT_GAP {
                // A pause, not jitter: keep the estimate and the target, and let the first
                // frame of the new talk-spurt wait for the usual prefill.
                self.prefilling = true;
                self.prefill_waited = 0;
                self.last_arrival = Some(now);
                return;
            }
            let delta_ms = gap.as_secs_f64() * 1000.0;
            let dev = (delta_ms - f64::from(FRAME_MS)).abs();
            let alpha = if dev > self.jitter_ms {
                0.25
            } else {
                1.0 / 32.0
            };
            self.jitter_ms += (dev - self.jitter_ms) * alpha;
            let frames = (self.jitter_ms / f64::from(FRAME_MS)).ceil() as usize + 1;
            self.target = frames.clamp(MIN_TARGET_FRAMES, MAX_TARGET_FRAMES);
        }
        self.last_arrival = Some(now);
    }

    fn warn_overflow(&mut self, now: Instant) {
        let due = match self.last_warn {
            None => true,
            Some(prev) => now.saturating_duration_since(prev) >= WARN_INTERVAL,
        };
        if due {
            self.last_warn = Some(now);
            self.warnings += 1;
            eprintln!(
                "[node-webrtc-rust mixer] source queue full ({QUEUE_CAPACITY} frames): merging oldest frames \
                 (overflow_compressed={}, pushed={}, played={}); the source is producing faster than real time",
                self.overflow_compressed, self.pushed, self.played
            );
        }
    }
}

fn samples(frame: &Frame) -> impl Iterator<Item = i16> + '_ {
    frame
        .pcm
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
}

/// RMS level of a frame in dBFS (`-inf` for digital silence).
pub fn frame_rms_dbfs(frame: &Frame) -> f64 {
    let mut sum = 0.0f64;
    let mut n = 0usize;
    for s in samples(frame) {
        let v = f64::from(s) / 32_768.0;
        sum += v * v;
        n += 1;
    }
    if n == 0 || sum == 0.0 {
        return f64::NEG_INFINITY;
    }
    10.0 * (sum / n as f64).log10()
}

/// Merges two consecutive frames into one 20 ms frame.
///
/// A frame below [`SILENCE_THRESHOLD_DBFS`] is dropped (the other is returned unchanged).
/// Otherwise the first `20 ms - 10 ms` come from `a`, then `a` and `b` are cross-faded
/// over 10 ms with a Hann (sin²) window ending on `b` (overlap-add, pitch unchanged).
pub fn merge_pair(a: &Frame, b: &Frame) -> Frame {
    if frame_rms_dbfs(a) < SILENCE_THRESHOLD_DBFS {
        return b.clone();
    }
    if frame_rms_dbfs(b) < SILENCE_THRESHOLD_DBFS {
        return a.clone();
    }
    let mut out = vec![0u8; FRAME_BYTES];
    out.copy_from_slice(&a.pcm);
    let start = (SAMPLES_PER_CHANNEL - CROSSFADE_SAMPLES) * 2;
    for i in 0..CROSSFADE_SAMPLES {
        let phase = std::f64::consts::FRAC_PI_2 * (i + 1) as f64 / CROSSFADE_SAMPLES as f64;
        let w = phase.sin().powi(2);
        for ch in 0..2 {
            let idx = (start + i * 2 + ch) * 2;
            let x = f64::from(i16::from_le_bytes([a.pcm[idx], a.pcm[idx + 1]]));
            let y = f64::from(i16::from_le_bytes([b.pcm[idx], b.pcm[idx + 1]]));
            let v = (x * (1.0 - w) + y * w).round().clamp(-32_768.0, 32_767.0) as i16;
            out[idx..idx + 2].copy_from_slice(&v.to_le_bytes());
        }
    }
    Frame::new(Bytes::from(out), None)
}

/// Replays `frame` with a linear fade from 0 dB to -6 dB across the 20 ms.
fn fade_out(frame: &Frame) -> Frame {
    let mut out = vec![0u8; FRAME_BYTES];
    for (i, s) in samples(frame).enumerate() {
        let t = (i / 2) as f64 / SAMPLES_PER_CHANNEL as f64;
        let gain = 1.0 - 0.5 * t;
        let v = (f64::from(s) * gain).round() as i16;
        out[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
    }
    Frame::new(Bytes::from(out), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_millis(FRAME_MS as u64);

    /// Constant-value frame; `id` is recoverable from any sample (and the last sample of a
    /// merged frame is the second frame's value).
    fn id_frame(id: u32) -> Frame {
        let value = (1_000 + id * 10) as i16;
        let mut pcm = vec![0u8; FRAME_BYTES];
        for i in 0..FRAME_BYTES / 2 {
            pcm[i * 2..i * 2 + 2].copy_from_slice(&value.to_le_bytes());
        }
        Frame::new(Bytes::from(pcm), None)
    }

    fn first_sample(f: &Frame) -> i16 {
        i16::from_le_bytes([f.pcm[0], f.pcm[1]])
    }

    fn last_sample(f: &Frame) -> i16 {
        let n = f.pcm.len();
        i16::from_le_bytes([f.pcm[n - 2], f.pcm[n - 1]])
    }

    fn id_of(value: i16) -> Option<u32> {
        let v = i32::from(value) - 1_000;
        if v >= 0 && v % 10 == 0 {
            Some((v / 10) as u32)
        } else {
            None
        }
    }

    /// xorshift64 for seeded jitter.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        /// Uniform in `[-max, max]` ms.
        fn jitter_ms(&mut self, max: i64) -> i64 {
            (self.next() % (2 * max as u64 + 1)) as i64 - max
        }
    }

    #[test]
    fn bunched_burst_plays_all_frames_in_order_without_compression() {
        let t0 = Instant::now();
        let mut q = ElasticFrameQueue::new();
        for id in 0..10 {
            q.push(id_frame(id), t0);
        }
        let mut played = Vec::new();
        for _ in 0..12 {
            let f = q.pop();
            if let Some(id) = id_of(last_sample(&f)) {
                played.push(id);
            }
        }
        assert_eq!(played, (0..10).collect::<Vec<_>>());
        let s = q.stats();
        assert_eq!(
            (s.pushed, s.played, s.compressed, s.overflow_compressed),
            (10, 10, 0, 0)
        );
        assert_eq!(s.depth, 0);
    }

    #[test]
    fn seeded_jitter_plays_each_frame_once_and_accounts_for_everything() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let t0 = Instant::now();
        let n = 500u32;
        let mut arrivals: Vec<(i64, u32)> = (0..n)
            .map(|i| (i64::from(i) * 20 + 200 + rng.jitter_ms(40), i))
            .collect();
        // The producer sends in order; jitter only delays (FIFO: no reordering).
        for i in 1..arrivals.len() {
            arrivals[i].0 = arrivals[i].0.max(arrivals[i - 1].0);
        }
        let mut q = ElasticFrameQueue::new();
        let mut next = 0usize;
        let mut last_id: Option<u32> = None;
        let mut real_outputs = 0u64;
        let mut concealed_outputs = 0u64;
        let ticks = (n as i64 * 20 + 1_000) / 20;
        for k in 0..ticks {
            let tick_ms = k * 20;
            while next < arrivals.len() && arrivals[next].0 <= tick_ms {
                let (t, id) = arrivals[next];
                q.push(id_frame(id), t0 + Duration::from_millis(t as u64));
                next += 1;
            }
            let f = q.pop();
            match id_of(last_sample(&f)) {
                Some(id) => {
                    real_outputs += 1;
                    if let Some(prev) = last_id {
                        assert!(id > prev, "frame {id} replayed or reordered after {prev}");
                    }
                    last_id = Some(id);
                }
                None => {
                    if first_sample(&f) != 0 {
                        concealed_outputs += 1;
                    }
                }
            }
        }
        let s = q.stats();
        assert_eq!(s.pushed, u64::from(n));
        assert_eq!(real_outputs, s.played);
        assert_eq!(concealed_outputs, s.concealed);
        assert_eq!(
            s.pushed,
            s.played + s.compressed + s.overflow_compressed + s.depth as u64
        );
        assert_eq!(s.depth, 0, "everything drained");
    }

    #[test]
    fn backlog_of_30_returns_to_target_by_compression() {
        let t0 = Instant::now();
        let mut q = ElasticFrameQueue::new();
        for id in 0..30 {
            q.push(id_frame(id), t0);
        }
        let mut ticks = 0;
        while q.depth() > q.target() + SHRINK_MARGIN_FRAMES && ticks < 100 {
            q.pop();
            ticks += 1;
        }
        assert!(ticks <= 100, "2 s = 100 ticks");
        let s = q.stats();
        assert!(s.compressed > 0);
        assert_eq!(s.pushed, s.played + s.compressed + s.depth as u64);
    }

    #[test]
    fn underrun_gap_conceals_once_then_silence() {
        let t0 = Instant::now();
        let mut q = ElasticFrameQueue::new();
        q.push(id_frame(1), t0);
        q.push(id_frame(2), t0);
        assert_eq!(id_of(last_sample(&q.pop())), Some(1));
        assert_eq!(id_of(last_sample(&q.pop())), Some(2));
        // 60 ms gap = 3 ticks.
        let c = q.pop();
        let v = i32::from(first_sample(&c));
        assert_eq!(
            v,
            i32::from(first_sample(&id_frame(2))),
            "fade starts at full level"
        );
        let end = i32::from(last_sample(&c));
        let full = i32::from(last_sample(&id_frame(2)));
        assert!(
            (end * 2 - full).abs() <= 4,
            "fades to -6 dB, got {end} of {full}"
        );
        assert_eq!(q.pop(), silence_frame());
        assert_eq!(q.pop(), silence_frame());
        assert_eq!(q.stats().concealed, 1);
    }

    #[test]
    fn overflow_of_80_bunched_frames_merges_counts_and_warns_once() {
        let t0 = Instant::now();
        let mut q = ElasticFrameQueue::new();
        for id in 0..80 {
            q.push(id_frame(id), t0);
        }
        let s = q.stats();
        assert_eq!(s.depth, QUEUE_CAPACITY);
        assert_eq!(s.overflow_compressed, 30);
        assert_eq!(q.warnings_emitted(), 1);
        assert_eq!(
            s.pushed,
            s.played + s.compressed + s.overflow_compressed + s.depth as u64
        );
    }

    #[test]
    fn empty_queue_after_three_empty_ticks_reprefills() {
        let t0 = Instant::now();
        let mut q = ElasticFrameQueue::new();
        q.push(id_frame(1), t0);
        q.push(id_frame(2), t0);
        q.pop();
        q.pop();
        for _ in 0..3 {
            q.pop();
        }
        // New frame alone is held back (prefill) until it has waited `target` ticks.
        q.push(id_frame(3), t0 + TICK);
        assert_eq!(q.pop(), silence_frame());
        assert_eq!(id_of(last_sample(&q.pop())), Some(3));
    }

    #[test]
    fn slim_drops_silent_frame_instead_of_blending() {
        let t0 = Instant::now();
        let mut q = ElasticFrameQueue::new();
        for id in 0..40 {
            if id % 2 == 1 {
                q.push(silence_frame(), t0);
            } else {
                q.push(id_frame(id), t0);
            }
        }
        for _ in 0..30 {
            q.pop();
        }
        let s = q.stats();
        assert!(s.compressed > 0);
        assert_eq!(s.pushed, s.played + s.compressed + s.depth as u64);
    }

    #[test]
    fn decode_errors_are_counted() {
        let t0 = Instant::now();
        let mut q = ElasticFrameQueue::new();
        q.note_decode_error("bad", t0);
        q.note_decode_error("bad", t0);
        assert_eq!(q.stats().decode_errors, 2);
    }

    #[test]
    fn merge_pair_cross_fades_between_the_two_frames() {
        let m = merge_pair(&id_frame(0), &id_frame(100));
        assert_eq!(first_sample(&m), first_sample(&id_frame(0)));
        assert_eq!(last_sample(&m), last_sample(&id_frame(100)));
    }

    #[test]
    fn target_grows_with_arrival_jitter_and_stays_clamped() {
        let t0 = Instant::now();
        let mut q = ElasticFrameQueue::new();
        for i in 0..50u64 {
            q.push(id_frame(0), t0 + Duration::from_millis(i * 20));
        }
        assert_eq!(q.target(), MIN_TARGET_FRAMES);
        let mut t = 2_000u64;
        for i in 0..100u64 {
            // Gaps stay below TALKSPURT_GAP (a longer gap is a pause, not jitter).
            t += if i % 2 == 0 { 0 } else { 180 };
            q.push(id_frame(0), t0 + Duration::from_millis(t));
        }
        assert_eq!(q.target(), MAX_TARGET_FRAMES);
    }

    /// Pushes `n` frames at 20 ms spacing from `start_ms`, each popped on its tick.
    fn steady(q: &mut ElasticFrameQueue, t0: Instant, start_ms: u64, n: u64, jitter: i64) {
        let mut rng = Rng(0xD1B5_4A32_D192_ED03);
        for i in 0..n {
            let at =
                (start_ms + i * 20) as i64 + if jitter > 0 { rng.jitter_ms(jitter) } else { 0 };
            q.push(
                id_frame(i as u32),
                t0 + Duration::from_millis(at.max(0) as u64),
            );
            q.pop();
        }
    }

    #[test]
    fn a_pause_does_not_count_as_jitter() {
        let t0 = Instant::now();
        let mut q = ElasticFrameQueue::new();
        steady(&mut q, t0, 0, 100, 5);
        assert_eq!(q.target(), MIN_TARGET_FRAMES);

        // 1 s pause: the queue runs dry and re-enters prefill; no frames arrive.
        for _ in 0..50 {
            q.pop();
        }
        // The first frame after the pause plays within `target` ticks.
        let resume = 3_000u64;
        q.push(id_frame(500), t0 + Duration::from_millis(resume));
        let mut ticks_to_first = 0u64;
        let mut next_id = 501u32;
        loop {
            ticks_to_first += 1;
            let f = q.pop();
            if id_of(last_sample(&f)) == Some(500) {
                break;
            }
            assert!(ticks_to_first <= 10, "first frame never played");
            q.push(
                id_frame(next_id),
                t0 + Duration::from_millis(resume + 20 * ticks_to_first),
            );
            next_id += 1;
        }
        assert!(
            ticks_to_first <= MIN_TARGET_FRAMES as u64,
            "first frame after the pause played after {ticks_to_first} ticks"
        );
        assert_eq!(
            q.target(),
            MIN_TARGET_FRAMES,
            "the pause did not raise the target"
        );
        steady(&mut q, t0, resume + 200, 100, 5);
        assert_eq!(q.target(), MIN_TARGET_FRAMES);
    }

    #[test]
    fn real_jitter_still_raises_the_target() {
        let t0 = Instant::now();
        let mut q = ElasticFrameQueue::new();
        steady(&mut q, t0, 0, 200, 30);
        assert!(
            q.target() > MIN_TARGET_FRAMES,
            "+/-30 ms jitter should raise the target, got {}",
            q.target()
        );
    }

    #[test]
    fn a_pause_re_enters_prefill_even_with_a_stale_frame_queued() {
        let t0 = Instant::now();
        let mut q = ElasticFrameQueue::new();
        q.push(id_frame(1), t0);
        q.push(id_frame(2), t0 + TICK);
        assert_eq!(id_of(last_sample(&q.pop())), Some(1));
        // The talker pauses for 1 s with a frame still queued, then a new spurt starts.
        q.push(id_frame(3), t0 + Duration::from_secs(1));
        assert_eq!(q.target(), MIN_TARGET_FRAMES);
        // Two frames queued (2 stale, 3 new): prefill is satisfied at once and order holds.
        assert_eq!(id_of(last_sample(&q.pop())), Some(2));
        assert_eq!(id_of(last_sample(&q.pop())), Some(3));
    }
}
