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
  /** Missing indices between consecutive detected frames. */
  gaps: number
  /** Frames that repeat the previous index or step backwards (delta > half the period). */
  duplicates: number
  ambiguous: number
  /** Silent frames after the first detected index. */
  silent: number
  /** Position in the input of the first numeric index, or -1. */
  firstIndexAt: number
}

/** Analyse detected indices, unwrapping mod 40. Leading silence is ignored. */
export function analyseSequence(indices: readonly DetectedIndex[]): SequenceAnalysis {
  const out: SequenceAnalysis = {
    played: 0,
    gaps: 0,
    duplicates: 0,
    ambiguous: 0,
    silent: 0,
    firstIndexAt: -1,
  }
  let prev = -1
  indices.forEach((idx, pos) => {
    if (idx === 'ambiguous') {
      if (out.firstIndexAt >= 0) out.ambiguous++
      return
    }
    if (idx === 'silence') {
      if (out.firstIndexAt >= 0) out.silent++
      return
    }
    if (prev < 0) {
      out.firstIndexAt = pos
      out.played = 1
      prev = idx
      return
    }
    const delta = (((idx - prev) % MARKER_PERIOD) + MARKER_PERIOD) % MARKER_PERIOD
    if (delta === 0 || delta > MARKER_PERIOD / 2) {
      out.duplicates++
      return
    }
    out.gaps += delta - 1
    out.played++
    prev = idx
  })
  return out
}
