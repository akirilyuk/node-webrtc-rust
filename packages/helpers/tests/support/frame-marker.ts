/**
 * Frame-index marker for audio continuity tests.
 *
 * Each 20 ms source frame (960 samples @ 48 kHz, stereo, L=R) carries a sine at
 * `f(i) = 400 + 50 * (i mod 40)` Hz, amplitude 0.3 FS, phase-continuous across frames.
 * The 40 candidate frequencies are exact multiples of the 50 Hz bin spacing of a 960-sample
 * window, so the Goertzel detector sees them as orthogonal. A cross-faded merge of two frames
 * or concealment shows up as `ambiguous`, a missing frame as a gap in the unwrapped sequence.
 *
 * Rust twin (same constants): crates/conference/tests/support/frame_marker.rs.
 */

export const MARKER_SAMPLE_RATE = 48_000
export const MARKER_SAMPLES_PER_FRAME = 960
export const MARKER_FRAME_BYTES = 3_840
export const MARKER_PERIOD = 40
export const MARKER_BASE_HZ = 400
export const MARKER_STEP_HZ = 50
export const MARKER_AMPLITUDE = 0.3
/** Argmax must beat the runner-up by this many dB, else the frame is `ambiguous`. */
export const MARKER_MIN_MARGIN_DB = 6
/** Frames whose RMS is below this level (dBFS) are `silence`. */
export const MARKER_SILENCE_DBFS = -50

export type DetectedIndex = number | 'ambiguous' | 'silence'

export function markerFrequencyHz(i: number): number {
  const k = ((i % MARKER_PERIOD) + MARKER_PERIOD) % MARKER_PERIOD
  return MARKER_BASE_HZ + MARKER_STEP_HZ * k
}

export interface MarkedFrameOptions {
  /** Shared phase state; omit for a frame that starts at phase 0 (not continuous). */
  state?: MarkerPhaseState
  /** Amplitude as a fraction of full scale (default 0.3). */
  amplitude?: number
}

export interface MarkerPhaseState {
  /** Radians, kept in [0, 2*PI). */
  phase: number
}

export function createMarkerPhaseState(): MarkerPhaseState {
  return { phase: 0 }
}

/** One 3840-byte frame (48 kHz stereo s16le, L=R) carrying marker index `i`. */
export function markedFrame(i: number, opts: MarkedFrameOptions = {}): Buffer {
  const state = opts.state ?? createMarkerPhaseState()
  const amp = (opts.amplitude ?? MARKER_AMPLITUDE) * 32_767
  const step = (2 * Math.PI * markerFrequencyHz(i)) / MARKER_SAMPLE_RATE
  const pcm = Buffer.alloc(MARKER_FRAME_BYTES)
  let phase = state.phase
  for (let n = 0; n < MARKER_SAMPLES_PER_FRAME; n++) {
    const s = Math.round(amp * Math.sin(phase))
    pcm.writeInt16LE(s, n * 4)
    pcm.writeInt16LE(s, n * 4 + 2)
    phase += step
  }
  state.phase = phase % (2 * Math.PI)
  return pcm
}

/** Generator that owns the phase state: `next(i)` returns frame `i`. */
export class FrameMarkerGenerator {
  private readonly state = createMarkerPhaseState()
  next(i: number, amplitude?: number): Buffer {
    return markedFrame(i, { state: this.state, amplitude })
  }
}

function goertzel(x: Float64Array, freq: number): number {
  const n = x.length
  const omega = (2 * Math.PI * Math.round((n * freq) / MARKER_SAMPLE_RATE)) / n
  const coeff = 2 * Math.cos(omega)
  let s1 = 0
  let s2 = 0
  for (let i = 0; i < n; i++) {
    const s0 = x[i]! + coeff * s1 - s2
    s2 = s1
    s1 = s0
  }
  return s1 * s1 + s2 * s2 - coeff * s1 * s2
}

/** Detect the marker index of one 3840-byte frame (left channel). */
export function detectIndex(frame: Buffer): DetectedIndex {
  const n = Math.min(MARKER_SAMPLES_PER_FRAME, Math.floor(frame.length / 4))
  if (n < MARKER_SAMPLES_PER_FRAME) return 'silence'
  const x = new Float64Array(n)
  let energy = 0
  for (let i = 0; i < n; i++) {
    x[i] = frame.readInt16LE(i * 4)
    energy += x[i]! * x[i]!
  }
  const rms = Math.sqrt(energy / n)
  const dbfs = 20 * Math.log10(Math.max(rms, 1e-9) / 32_768)
  if (dbfs < MARKER_SILENCE_DBFS) return 'silence'
  let best = -1
  let bestP = -1
  let second = -1
  for (let k = 0; k < MARKER_PERIOD; k++) {
    const p = goertzel(x, markerFrequencyHz(k))
    if (p > bestP) {
      second = bestP
      bestP = p
      best = k
    } else if (p > second) {
      second = p
    }
  }
  const marginDb = 10 * Math.log10(Math.max(bestP, 1e-9) / Math.max(second, 1e-9))
  return marginDb >= MARKER_MIN_MARGIN_DB ? best : 'ambiguous'
}

export interface SequenceAnalysis {
  /** Frames that advanced the unwrapped sequence (first frame included). */
  played: number
  /** Marker indices missing between heard frames and not explained by an unknown frame. */
  gaps: number
  /** An index repeated back-to-back (replayed or concealed frame) before a later marker. */
  duplicates: number
  /** Repeats after the last advancing marker (the end-of-stream concealment tail). */
  trailingDuplicates: number
  /** Indices that went backwards (delta > half the period). */
  reordered: number
  /** Ambiguous frames between the first and the last marker. */
  ambiguous: number
  /** Silent frames after the first detected index (interior and trailing). */
  silent: number
  /** Silent frames between the first and the last marker. */
  interiorSilence: number
  /** Position in the input of the first numeric index, or -1. */
  firstIndexAt: number
  /**
   * Marker distance walked from the first to the last recognised frame: the sum of the
   * unwrapped forward steps between consecutive advancing markers (a long forward step counts
   * at its unwrapped length). A stream of N source frames has a span of at most N - 1, so a
   * backward step (a replay) that a merge budget lets through as forward shows up as a span
   * about 40 too long.
   */
  span: number
}

export interface AnalyseOptions {
  /**
   * Output frames the mixer reports as merged (`mixStats`: `compressed + overflowCompressed +
   * listenerCatchupCompressed + listenerSkipped`). Each merge swallows one marker index, so a
   * run of merged (ambiguous) frames advances the index faster than one per frame.
   *
   * The sequence is unwrapped mod 40, which is only unambiguous for a forward step of at most 20.
   * A longer run of merges or concealed frames steps further than that (11 merged frames moved
   * the marker 23 ahead in a stalled run). A step above 20 therefore counts as forward when the
   * indices it skips beyond its unknown frames are paid for by the merges not used yet, and at
   * most one per frame of the step. Anything else above 20 is `reordered`. Default 0: no merge
   * is accounted for, so only a run of unknown frames can explain a long forward step.
   */
  mergedFrames?: number
  /**
   * How many of `mergedFrames` are listener catch-up merges (`listenerCatchupCompressed`).
   * A catch-up output frame cross-fades two media ticks, and each tick may itself be a
   * queue-compressed pair (`compressed`: `pop` merges at most two queued frames). When both
   * kinds occur in a run, one output frame carries up to four source frames, so it can absorb
   * up to three indices, not one. `overflowCompressed` can stack more than two frames into one
   * tick (each overflow merges the queue front again), so with overflow this cap can be too
   * strict (a false `reordered`), never too loose. Default 0: every frame of a step absorbs at
   * most one index.
   */
  listenerCatchupFrames?: number
}

/**
 * Indices one output frame can absorb when listener catch-up merges two compressed ticks
 * (pop compression; overflow can stack more).
 */
const MAX_STACKED_MERGE_INDICES = 3

/**
 * Analyse detected indices, unwrapping mod 40. Leading silence is ignored. An ambiguous or
 * interior-silent frame stands in for one unknown frame, so it explains one missing index
 * (the Rust twin `analyze` in `crates/conference/tests/support/frame_marker.rs` does the same;
 * it still treats every step above 20 as `reordered`, see `AnalyseOptions.mergedFrames` and
 * `AnalyseOptions.listenerCatchupFrames`).
 */
export function analyseSequence(
  indices: readonly DetectedIndex[],
  opts: AnalyseOptions = {},
): SequenceAnalysis {
  const out: SequenceAnalysis = {
    played: 0,
    gaps: 0,
    duplicates: 0,
    trailingDuplicates: 0,
    reordered: 0,
    ambiguous: 0,
    silent: 0,
    interiorSilence: 0,
    firstIndexAt: -1,
    span: 0,
  }
  let lastMarker = -1
  indices.forEach((idx, pos) => {
    if (typeof idx === 'number') lastMarker = pos
  })
  let prev = -1
  let unknown = 0
  let pendingDups = 0
  let mergesLeft = opts.mergedFrames ?? 0
  const catchup = Math.min(opts.listenerCatchupFrames ?? 0, mergesLeft)
  // Listener catch-up merges stacked on queue compressions: one frame can absorb three indices.
  const perFrame = catchup > 0 && mergesLeft - catchup > 0 ? MAX_STACKED_MERGE_INDICES : 1
  indices.forEach((idx, pos) => {
    if (idx === 'silence' && out.firstIndexAt >= 0) out.silent++
    if (pos > lastMarker) return
    if (idx === 'ambiguous') {
      if (prev >= 0) {
        out.ambiguous++
        unknown++
      }
      return
    }
    if (idx === 'silence') {
      if (prev >= 0) {
        out.interiorSilence++
        unknown++
      }
      return
    }
    if (prev < 0) {
      out.firstIndexAt = pos
      out.played = 1
      prev = idx
      return
    }
    const delta = (((idx - prev) % MARKER_PERIOD) + MARKER_PERIOD) % MARKER_PERIOD
    if (delta === 0) {
      pendingDups++
      return
    }
    // Indices this step skips that no unknown frame (ambiguous / interior silence) stands in for.
    const unexplained = Math.max(0, delta - 1 - unknown)
    // A step above half the period is ambiguous mod 40 (forward by `delta` or back by
    // `40 - delta`). It is forward when the unknown frames cover it, or when unused merges pay
    // for the rest and each frame of the step absorbed at most `perFrame` indices (one, or
    // three when listener catch-up merges two queue-compressed ticks).
    const longForward =
      delta > MARKER_PERIOD / 2 && unexplained <= Math.min(mergesLeft, perFrame * (unknown + 1))
    if (delta > MARKER_PERIOD / 2 && !longForward) {
      out.reordered++
    } else {
      if (longForward) mergesLeft -= unexplained
      out.gaps += unexplained
      out.played++
      out.span += delta
      prev = idx
      out.duplicates += pendingDups
      pendingDups = 0
    }
    unknown = 0
  })
  out.trailingDuplicates = pendingDups
  return out
}
