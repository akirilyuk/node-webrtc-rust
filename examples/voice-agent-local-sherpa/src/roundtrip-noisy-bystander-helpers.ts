/**
 * Pure helpers of `roundtrip-noisy-bystander.ts` (no models needed; Vitest in
 * `roundtrip-noisy-bystander.test.ts`).
 */

import { readFileSync } from 'fs'

import { SPEECH_EVENT_TYPE } from '@node-webrtc-rust/sdk/voice'

import { DEFAULT_BARGE_PHRASE_MIN_SIMILARITY } from './roundtrip-barge-in-helpers.js'
import { wordSimilarity } from './roundtrip-counting.js'

/** Product default of `speakerGate.checkSpeechMs` (G1): the verdict needs this much speech. */
export const CHECK_SPEECH_MS = 1500
/**
 * `vad.bargeIn.minSttPartialTokens` the roundtrip sets on its agent (the product default, 2). With
 * `requireSttPartial` a barge-in waits for the first partial with at least this many tokens.
 */
export const MIN_STT_PARTIAL_TOKENS = 2
/** A barge-in may come at most this long after the later of the verdict and the qualifying partial. */
export const BARGE_SLACK_MS = 300
/** Absolute cap on barge-in delay after the caller's `user_speaking_start`. */
export const BARGE_ABS_CAP_MS = 3000
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

/**
 * Token count as the product counts it (`stt_partial_token_count` in crates/speech config.rs):
 * whitespace-separated tokens with at least one alphanumeric character.
 */
export function sttPartialTokenCount(text: string): number {
  return text.split(/\s+/).filter((tok) => tok.length > 0 && /[\p{L}\p{N}]/u.test(tok)).length
}

/**
 * When a caller's barge-in is due, from events the product emits for that utterance: the later of
 * speech start + checkSpeechMs (speaker verdict) and the first `user_speech_partial` on the same
 * `utteranceId` with at least {@link MIN_STT_PARTIAL_TOKENS} tokens (`requireSttPartial`).
 * `undefined` when the utterance never had a qualifying partial.
 */
export function expectedBargeAtMs(start: Seen, seen: Seen[]): number | undefined {
  const partial = seen.find(
    (e) =>
      e.type === SPEECH_EVENT_TYPE.userSpeechPartial &&
      e.utteranceId === start.utteranceId &&
      sttPartialTokenCount(e.text ?? '') >= MIN_STT_PARTIAL_TOKENS,
  )
  if (partial == null) return undefined
  return Math.max(start.atMs + CHECK_SPEECH_MS, partial.atMs)
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

/** Slack before the first judged turn: the agent's VAD can open a little before the voice. */
export const FIRST_SLOT_LEAD_MS = 300

/**
 * Matches each scene turn to the utterance the agent heard for it: the first unused
 * `user_speaking_start` inside the turn's slot. Slots split each gap between two turns in the
 * middle, because the VAD can open in the babble shortly before a voice starts. A turn whose
 * audio merged into the previous utterance gets none. Everything after this step pairs events by
 * `utteranceId`, never by time.
 */
export function matchTurnUtterances(turns: Turn[], seen: Seen[]): (Seen | undefined)[] {
  const starts = seen.filter(
    (e) => e.type === SPEECH_EVENT_TYPE.userSpeakingStart && e.utteranceId != null,
  )
  const used = new Set<string>()
  const boundary = (i: number): number => {
    if (i <= 0) return turns[0]!.startMs - FIRST_SLOT_LEAD_MS
    if (i >= turns.length) return Number.POSITIVE_INFINITY
    return (turns[i - 1]!.endMs + turns[i]!.startMs) / 2
  }
  return turns.map((_turn, i) => {
    const from = boundary(i)
    const to = boundary(i + 1)
    const start = starts.find((e) => !used.has(e.utteranceId!) && e.atMs >= from && e.atMs < to)
    if (start != null) used.add(start.utteranceId!)
    return start
  })
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
  // A bystander never barges in: any barge_in whose utterance's final is labelled other fails,
  // whether or not that utterance matched a scene turn.
  for (const b of barges) {
    if (b.utteranceId != null && finals.get(b.utteranceId)?.speaker === 'other') {
      failures.push(
        `${label}: barge_in at ${b.atMs} ms for utterance ${b.utteranceId} whose final is speaker other`,
      )
    }
  }
  for (const b of barges) {
    if (b.utteranceId == null)
      failures.push(`${label}: barge_in at ${b.atMs} ms without utteranceId`)
  }
  const bargesOf = (id: string): Seen[] => barges.filter((b) => b.utteranceId === id)
  const starts = matchTurnUtterances(turns, seen)
  let bystandersHeard = 0
  turns.forEach((turn, i) => {
    const tag = `${label} turn ${i} (${turn.role} ${turn.kind} "${turn.text.slice(0, 30)}")`
    const start = starts[i]
    const id = start?.utteranceId
    const final = id != null ? finals.get(id) : undefined
    if (turn.role === 'bystander') {
      if (id != null && bargesOf(id).length > 0) {
        failures.push(`${tag}: barge_in for the bystander utterance ${id}`)
      }
      if (final != null) {
        if (final.speaker !== 'other') {
          failures.push(`${tag}: final speaker ${String(final.speaker)}, expected other`)
        } else {
          bystandersHeard += 1
        }
      }
      return
    }
    if (start == null || id == null) {
      failures.push(`${tag}: caller speech start not detected`)
      return
    }
    const barge = bargesOf(id)[0]
    if (barge == null) {
      failures.push(`${tag}: no barge_in`)
    } else {
      const delay = barge.atMs - start.atMs
      const expected = expectedBargeAtMs(start, seen)
      console.log(
        `${tag}: barge-in ${delay} ms after speech start, ${expected != null ? barge.atMs - expected : 'n/a'} ms after expected`,
      )
      if (expected == null) {
        failures.push(
          `${tag}: barge_in without a qualifying partial (>= ${MIN_STT_PARTIAL_TOKENS} tokens) on ${id}`,
        )
      } else if (barge.atMs - expected > BARGE_SLACK_MS) {
        failures.push(
          `${tag}: barge-in ${barge.atMs - expected} ms after expected (max of verdict and qualifying partial) > ${BARGE_SLACK_MS}`,
        )
      }
      if (delay > BARGE_ABS_CAP_MS) {
        failures.push(`${tag}: barge-in ${delay} ms after speech start > cap ${BARGE_ABS_CAP_MS}`)
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
