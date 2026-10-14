/**
 * Pure helpers for the C1 timeout smoke (`roundtrip-c1-timeouts.ts`): deterministic noise
 * frames and per-phase evaluators. Unit-tested without Sherpa.
 */

import { SPEECH_EVENT_TYPE } from '@node-webrtc-rust/sdk/voice'

import { PCM_FRAME_DURATION_MS, PCM_FULL_FRAME_BYTES } from '../../shared/pcm-streaming.js'

export interface C1SmokeEvent {
  type: string
  /** Milliseconds since the phase recording started. */
  atMs: number
  text?: string
}

export interface C1PhaseResult {
  passed: boolean
  failures: string[]
  /** `user_stt_not_found` times relative to the phase anchor (noise end / sender stop). */
  notFoundAfterMs: number[]
  finals: string[]
}

/** Phase A: not_found must land this long after the noise ends (frames keep flowing). */
export const PHASE_A_MIN_MS = 1500
export const PHASE_A_MAX_MS = 6500
/** Phase B: sender stops mid-speech; wall net must not fire before the hard timeout. */
export const PHASE_B_MIN_MS = 10_000
export const PHASE_B_MAX_MS = 15_000

/** Small deterministic PRNG (mulberry32). */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

/**
 * Broadband white noise as 20 ms interleaved stereo s16le @ 48 kHz frames. Deterministic for a
 * given seed. Uniform in ±amplitude, so RMS is about amplitude / sqrt(3).
 */
export function createNoiseFrames(durationMs: number, seed = 1, amplitude = 8000): Buffer[] {
  const rand = mulberry32(seed)
  const frameCount = Math.ceil(durationMs / PCM_FRAME_DURATION_MS)
  const samplesPerChannel = PCM_FULL_FRAME_BYTES / 4
  const frames: Buffer[] = []
  for (let f = 0; f < frameCount; f++) {
    const buf = Buffer.alloc(PCM_FULL_FRAME_BYTES)
    for (let i = 0; i < samplesPerChannel; i++) {
      const sample = Math.round((rand() * 2 - 1) * amplitude)
      buf.writeInt16LE(sample, i * 4)
      buf.writeInt16LE(sample, i * 4 + 2)
    }
    frames.push(buf)
  }
  return frames
}

function finalTexts(events: C1SmokeEvent[]): string[] {
  return events
    .filter((e) => e.type === SPEECH_EVENT_TYPE.userSpeechFinal)
    .map((e) => (e.text ?? '').trim())
    .filter((t) => t.length > 0)
}

function notFoundTimes(events: C1SmokeEvent[], anchorMs: number): number[] {
  return events
    .filter((e) => e.type === SPEECH_EVENT_TYPE.userSttNotFound)
    .map((e) => e.atMs - anchorMs)
}

/**
 * Phase A: noise burst, then silence frames keep flowing. Exactly one `user_stt_not_found`
 * between 1.5 and 6.5 s after the noise ends (any close path), and no non-empty final.
 */
export function evaluatePhaseA(params: {
  events: C1SmokeEvent[]
  noiseEndAtMs: number
}): C1PhaseResult {
  const failures: string[] = []
  const notFoundAfterMs = notFoundTimes(params.events, params.noiseEndAtMs)
  const finals = finalTexts(params.events)
  if (notFoundAfterMs.length !== 1) {
    failures.push(`A: expected exactly 1 user_stt_not_found, got ${notFoundAfterMs.length}`)
  } else {
    const t = notFoundAfterMs[0]!
    if (t < PHASE_A_MIN_MS || t > PHASE_A_MAX_MS) {
      failures.push(
        `A: user_stt_not_found ${t} ms after noise end, expected ${PHASE_A_MIN_MS}-${PHASE_A_MAX_MS} ms`,
      )
    }
  }
  if (finals.length > 0) {
    failures.push(`A: unexpected non-empty user_speech_final: ${JSON.stringify(finals)}`)
  }
  return { passed: failures.length === 0, failures, notFoundAfterMs, finals }
}

/**
 * Phase B: sender stops mid-speech and writes nothing. No `user_stt_not_found` earlier than
 * 10 s after the stop, exactly one by 15 s.
 */
export function evaluatePhaseB(params: {
  events: C1SmokeEvent[]
  stopAtMs: number
}): C1PhaseResult {
  const failures: string[] = []
  const notFoundAfterMs = notFoundTimes(params.events, params.stopAtMs)
  const finals = finalTexts(params.events)
  const early = notFoundAfterMs.filter((t) => t < PHASE_B_MIN_MS)
  if (early.length > 0) {
    failures.push(
      `B: user_stt_not_found ${early[0]} ms after the sender stopped, earlier than ${PHASE_B_MIN_MS} ms (VAD is in speech)`,
    )
  }
  const inWindow = notFoundAfterMs.filter((t) => t <= PHASE_B_MAX_MS)
  if (inWindow.length !== 1) {
    failures.push(
      `B: expected exactly 1 user_stt_not_found by ${PHASE_B_MAX_MS} ms, got ${inWindow.length}`,
    )
  }
  if (notFoundAfterMs.length > 1) {
    failures.push(
      `B: expected a single user_stt_not_found in the phase, got ${notFoundAfterMs.length}`,
    )
  }
  return { passed: failures.length === 0, failures, notFoundAfterMs, finals }
}

const NUMBER_WORDS = ['one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight', 'nine', 'ten']

function numberTokens(text: string): string[] {
  const out: string[] = []
  for (const raw of text.toLowerCase().split(/[^a-z0-9]+/)) {
    if (!raw) continue
    if (NUMBER_WORDS.includes(raw)) out.push(raw)
    else if (/^(10|[1-9])$/.test(raw)) out.push(NUMBER_WORDS[Number(raw) - 1]!)
  }
  return out
}

/** Longest greedy in-order run of number words one..ten, and whether one of seven..ten was heard. */
export function countOrderedNumberWords(text: string): { count: number; lateWord: boolean } {
  let last = -1
  let count = 0
  let lateWord = false
  for (const tok of numberTokens(text)) {
    const idx = NUMBER_WORDS.indexOf(tok)
    if (idx > last) {
      count += 1
      last = idx
      if (idx >= 6) lateWord = true
    }
  }
  return { count, lateWord }
}

export const PHASE_C_MIN_NUMBER_WORDS = 8

/**
 * Phase C: a 4.5 s gap with no frames mid-phrase. No `user_stt_not_found`; finals joined hold
 * at least 8 number words in order, including one spoken after the gap.
 */
export function evaluatePhaseC(params: { events: C1SmokeEvent[] }): C1PhaseResult {
  const failures: string[] = []
  const notFoundAfterMs = notFoundTimes(params.events, 0)
  const finals = finalTexts(params.events)
  if (notFoundAfterMs.length > 0) {
    failures.push(`C: unexpected user_stt_not_found at ${notFoundAfterMs.join(', ')} ms`)
  }
  const { count, lateWord } = countOrderedNumberWords(finals.join(' '))
  if (count < PHASE_C_MIN_NUMBER_WORDS) {
    failures.push(
      `C: only ${count} ordered number words in finals ${JSON.stringify(finals)}, expected at least ${PHASE_C_MIN_NUMBER_WORDS}`,
    )
  }
  if (!lateWord) {
    failures.push(
      `C: no word after the gap (seven/eight/nine/ten) in finals ${JSON.stringify(finals)}`,
    )
  }
  return { passed: failures.length === 0, failures, notFoundAfterMs, finals }
}
