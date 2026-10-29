/**
 * Pure helpers of `roundtrip-noisy-bystander.ts` (no models needed; Vitest in
 * `roundtrip-noisy-bystander.test.ts`).
 */

import { readFileSync } from 'fs'

import { SPEECH_EVENT_TYPE } from '@node-webrtc-rust/sdk/voice'

import { DEFAULT_BARGE_PHRASE_MIN_SIMILARITY } from './roundtrip-barge-in-helpers.js'
import { wordSimilarity } from './roundtrip-counting.js'

/** Product defaults of `speakerGate` (G1): the delay bound is checkSpeechMs + this margin. */
export const CHECK_SPEECH_MS = 1500
export const BARGE_DELAY_MARGIN_MS = 150
export const BYSTANDER_RELATIVE_DB = -15
export const QUIET_CALLER_DBFS = -26
export const WARMUP_S = 0.6

export interface Turn {
  role: 'main' | 'bystander'
  startMs: number
  endMs: number
  text: string
  kind: 'qualified' | 'long' | 'background'
}

export interface Seen {
  type: string
  atMs: number
  text?: string
  utteranceId?: string
  speaker?: string
}

/** Interleaved stereo s16le @ 48 kHz payload of a wav file (the `data` chunk). */
export function readWavPcm(path: string): Buffer {
  const buf = readFileSync(path)
  let pos = 12
  while (pos + 8 <= buf.length) {
    const id = buf.toString('ascii', pos, pos + 4)
    const size = buf.readUInt32LE(pos + 4)
    if (id === 'data') return buf.subarray(pos + 8, Math.min(buf.length, pos + 8 + size))
    pos += 8 + size + (size % 2)
  }
  throw new Error(`no data chunk in ${path}`)
}

/** Pure evaluation of one pass; exported for the evaluator unit test. */
export function evaluateTurns(turns: Turn[], seen: Seen[], label: string): string[] {
  const failures: string[] = []
  const finals = new Map<string, Seen>()
  for (const e of seen) {
    if (e.type === SPEECH_EVENT_TYPE.userSpeechFinal && e.utteranceId != null) {
      finals.set(e.utteranceId, e)
    }
  }
  const barges = seen.filter((e) => e.type === SPEECH_EVENT_TYPE.bargeIn)
  let bystandersHeard = 0
  turns.forEach((turn, i) => {
    const tag = `${label} turn ${i} (${turn.role} ${turn.kind} "${turn.text.slice(0, 30)}")`
    const start = seen.find(
      (e) =>
        e.type === SPEECH_EVENT_TYPE.userSpeakingStart &&
        e.atMs >= turn.startMs - 300 &&
        e.atMs <= turn.endMs + 300,
    )
    const final = start?.utteranceId != null ? finals.get(start.utteranceId) : undefined
    if (turn.role === 'bystander') {
      const hit = barges.filter((b) => b.atMs >= turn.startMs && b.atMs <= turn.endMs + 1000)
      if (hit.length > 0) failures.push(`${tag}: barge_in during a bystander turn`)
      if (final != null) {
        if (final.speaker !== 'other') {
          failures.push(`${tag}: final speaker ${String(final.speaker)}, expected other`)
        } else {
          bystandersHeard += 1
        }
      }
      return
    }
    const barge = barges.find(
      (b) => b.atMs >= turn.startMs && b.atMs <= Math.max(turn.endMs, turn.startMs + 2200),
    )
    if (start == null) {
      failures.push(`${tag}: caller speech start not detected`)
      return
    }
    if (barge == null) {
      failures.push(`${tag}: no barge_in`)
    } else {
      const delay = barge.atMs - start.atMs
      console.log(`${tag}: barge-in ${delay} ms after speech start`)
      if (delay > CHECK_SPEECH_MS + BARGE_DELAY_MARGIN_MS) {
        failures.push(
          `${tag}: barge-in ${delay} ms after speech start > ${CHECK_SPEECH_MS + BARGE_DELAY_MARGIN_MS}`,
        )
      }
    }
    if (final == null) {
      failures.push(`${tag}: no user_speech_final`)
      return
    }
    if (final.speaker !== 'caller') {
      failures.push(`${tag}: final speaker ${String(final.speaker)}, expected caller`)
    }
    if (turn.kind === 'qualified') {
      const sim = wordSimilarity(turn.text, final.text ?? '')
      console.log(`${tag}: recognized "${final.text ?? ''}" similarity ${sim.toFixed(2)}`)
      if (sim < DEFAULT_BARGE_PHRASE_MIN_SIMILARITY) {
        failures.push(`${tag}: recognized "${final.text ?? ''}" similarity ${sim.toFixed(2)}`)
      }
    }
  })
  console.log(`${label}: bystander finals tagged other: ${bystandersHeard}`)
  if (bystandersHeard === 0) {
    failures.push(`${label}: no bystander utterance was detected and tagged other (vacuous pass)`)
  }
  return failures
}
