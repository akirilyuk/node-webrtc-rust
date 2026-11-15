//! Decode the whole utterance at `finalize_utterance`.
//!
//! A streaming zipformer decodes whole chunks only: `is_ready` turns true once the stream holds
//! a chunk plus the model's look-ahead past the last decoded frame, and every `decode` moves the
//! decoded position by one chunk. `FINALIZE_TAIL_PADDING_MS` (640 ms) is shorter than one chunk of
//! the Kroko English model (about 1.3 s), so up to about 0.8 s of speech after the last full
//! chunk was never decoded and the final lost its last words ("delta seven eight nine" ->
//! "Delta seven").
//!
//! The flush measures the chunk grid of each recognizer once on a probe stream of zeros, using
//! only `accept_waveform` / `is_ready` / `decode`: `first_ready` is the samples a fresh stream
//! needs for its first step, `step` the samples each later step needs. After a drain (nothing
//! ready) the undecoded part of the stream is shorter than `first_ready`, so
//! `ceil(first_ready / step)` further steps move the decoded position past every sample the
//! stream holds, real audio included. The flush appends zeros in 10 ms pieces and decodes until
//! it ran that many steps, bounded by `FINALIZE_MAX_EXTRA_PADDING_MS`, then calls
//! `input_finished` and drains.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use sherpa_onnx::{OnlineRecognizer, OnlineStream};

use crate::loader::voice_debug;

const SAMPLE_RATE: i32 = 16_000;
/// Zeros appended per flush iteration (10 ms at 16 kHz).
const FLUSH_PIECE_SAMPLES: usize = 160;
/// Decode steps per drain loop (same cap as every other decode loop in this crate).
const MAX_DRAIN_STEPS: u32 = 64;
/// Probe bound: a model whose chunk needs more than this many seconds of audio has no geometry.
const PROBE_MAX_SAMPLES: usize = SAMPLE_RATE as usize * 10;

/// Upper bound on the zeros the flush appends after `FINALIZE_TAIL_PADDING_MS`. Measured need:
/// Kroko English 2025-08-06 about 1.9 s at most (two steps of ~0.64 s after a drain, the first
/// step needing up to one chunk + look-ahead); the bound keeps a 2x margin.
pub const FINALIZE_MAX_EXTRA_PADDING_MS: usize = 4_000;

static FLUSH_BOUND_LOGGED: AtomicBool = AtomicBool::new(false);
static UTTERANCE_DECODE_STEPS: AtomicU64 = AtomicU64::new(0);

/// Per-stream decode steps on an utterance's own audio plus the fixed tail padding: the push
/// decodes and the flush's first drain, not the steps on the flush's extra zeros nor the
/// batched decode thread's rounds (test probe).
pub fn sherpa_utterance_decode_steps() -> u64 {
    UTTERANCE_DECODE_STEPS.load(Ordering::SeqCst)
}

pub fn reset_sherpa_utterance_decode_steps() {
    UTTERANCE_DECODE_STEPS.store(0, Ordering::SeqCst);
}

pub(crate) fn count_utterance_decode_steps(steps: u32) {
    UTTERANCE_DECODE_STEPS.fetch_add(u64::from(steps), Ordering::SeqCst);
}

/// Chunk grid of one recognizer, in 16 kHz samples (multiples of 10 ms).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkGeometry {
    /// Samples a fresh stream needs before its first decode step.
    pub first_ready: usize,
    /// Samples each following decode step needs.
    pub step: usize,
}

impl ChunkGeometry {
    /// Steps after a drain that move the decoded position past everything the stream holds.
    pub fn flush_steps(&self) -> u32 {
        self.first_ready.div_ceil(self.step).max(1) as u32
    }
}

/// What one flush did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlushOutcome {
    /// Decode steps run by the flush, the drains included.
    pub steps: u32,
    /// Zeros appended after the tail padding, in samples.
    pub extra_samples: usize,
    /// The step target was not reached within `FINALIZE_MAX_EXTRA_PADDING_MS`.
    pub bound_hit: bool,
}

/// Measure the chunk grid on a probe stream of zeros. `None` when the model never becomes ready
/// within 10 s of audio (the flush then only drains, as before).
pub fn probe_chunk_geometry(recognizer: &OnlineRecognizer) -> Option<ChunkGeometry> {
    let stream = recognizer.create_stream();
    let piece = vec![0.0_f32; FLUSH_PIECE_SAMPLES];
    let feed_until_ready = |limit: usize| -> Option<usize> {
        let mut fed = 0usize;
        while !recognizer.is_ready(&stream) {
            if fed >= limit {
                return None;
            }
            stream.accept_waveform(SAMPLE_RATE, &piece);
            fed += FLUSH_PIECE_SAMPLES;
        }
        Some(fed)
    };
    let first_ready = feed_until_ready(PROBE_MAX_SAMPLES)?;
    recognizer.decode(&stream);
    // The step is the largest gap between two ready points over a few steps (a stream that is
    // still ready right after a decode needs no new samples for that step).
    let mut step = 0usize;
    for _ in 0..3 {
        let fed = feed_until_ready(PROBE_MAX_SAMPLES)?;
        step = step.max(fed);
        recognizer.decode(&stream);
    }
    (first_ready > 0 && step > 0).then_some(ChunkGeometry { first_ready, step })
}

fn drain(recognizer: &OnlineRecognizer, stream: &OnlineStream) -> u32 {
    let mut steps = 0u32;
    while steps < MAX_DRAIN_STEPS && recognizer.is_ready(stream) {
        recognizer.decode(stream);
        steps += 1;
    }
    steps
}

/// Decode everything `stream` holds and finish its input. The caller has already fed
/// `FINALIZE_TAIL_PADDING_MS` of zeros. Runs on the caller's thread with the recognizer held.
pub fn flush_finalize_tail(
    recognizer: &OnlineRecognizer,
    stream: &OnlineStream,
    geometry: Option<ChunkGeometry>,
) -> FlushOutcome {
    let mut outcome = FlushOutcome {
        steps: drain(recognizer, stream),
        ..FlushOutcome::default()
    };
    count_utterance_decode_steps(outcome.steps);
    if let Some(geometry) = geometry {
        let needed = geometry.flush_steps();
        let max_extra = SAMPLE_RATE as usize * FINALIZE_MAX_EXTRA_PADDING_MS / 1000;
        let piece = vec![0.0_f32; FLUSH_PIECE_SAMPLES];
        let mut flushed = 0u32;
        loop {
            flushed += drain(recognizer, stream);
            if flushed >= needed {
                break;
            }
            if outcome.extra_samples >= max_extra {
                outcome.bound_hit = true;
                if !FLUSH_BOUND_LOGGED.swap(true, Ordering::Relaxed) {
                    voice_debug(format!(
                        "sherpa finalize flush bound hit extra_padding_ms={} steps={flushed} needed={needed}",
                        outcome.extra_samples * 1000 / SAMPLE_RATE as usize
                    ));
                }
                break;
            }
            stream.accept_waveform(SAMPLE_RATE, &piece);
            outcome.extra_samples += FLUSH_PIECE_SAMPLES;
        }
        outcome.steps += flushed;
    }
    stream.input_finished();
    outcome.steps += drain(recognizer, stream);
    outcome
}
