/**
 * Pure helpers for the slow-final smoke (`roundtrip-slow-final.ts`): timing and order checks over
 * recorded echo-side speech events plus speaker-side send times. Unit-tested without Sherpa.
 *
 * All times (`atMs`, `sentAtMs`) are milliseconds on one clock, the phase clock of the harness.
 */

import { SPEECH_EVENT_TYPE } from '@node-webrtc-rust/sdk/voice'

/** Echo-side speech event as recorded by the smoke. */
export interface SlowFinalEvent {
  type: string
  /** Milliseconds since the phase clock started. */
  atMs: number
  utteranceId?: string
  text?: string
}

/** One speaker phrase: when its first voiced frame was sent, and a word its final must contain. */
export interface PhraseRef {
  /** Short name used in failure reasons, e.g. `P2`. */
  label: string
  sentAtMs: number
  /** When set, the phrase's final must contain this word (case-insensitive whole token). */
  word?: string
}

/** A frame of audio the speaker received from the echo agent. */
export interface ReceivedFrame {
  atMs: number
  /** RMS of the frame, 0..1 (s16 sample / 32768). */
  rms: number
}

export type CheckResult = { ok: true } | { ok: false; reason: string }

const OK: CheckResult = { ok: true }

function fail(reason: string): CheckResult {
  return { ok: false, reason }
}

/**
 * The echo agent's inbound path no longer waits for STT, so a new utterance's
 * `user_speaking_start` follows the first voiced frame within VAD min-speech plus transport.
 * On the old code it waited for the pending final: about the whole 6 s delay.
 */
export const NEXT_TURN_START_MAX_MS = 300
/** `barge_in` (VAD barge-in) after the first voiced frame of the interrupting phrase. */
export const BARGE_MAX_MS = 300
/** `agent_speaking_end` after `barge_in` (the TTS flush). */
export const SPEAKING_END_AFTER_BARGE_MAX_MS = 300
/** `SPEECH_STT_TEST_FINAL_DELAY_MS` the echo agent runs with. */
export const FINAL_DELAY_MS = 6000
/** Semantic barge-in may wait for the earlier final (FINAL_DELAY_MS) plus this much. */
export const SEMANTIC_BARGE_SLACK_MS = 1500
/** A received frame with RMS above this counts as audible (normalized 0..1). */
export const AUDIBLE_RMS_THRESHOLD = 0.02
/** After `barge_in`, received audio must be silent from this many ms on (network and jitter buffer)... */
export const QUIET_WINDOW_START_AFTER_BARGE_MS = 500
/** ...until this many ms after `barge_in`. */
export const QUIET_WINDOW_END_AFTER_BARGE_MS = 1500

/** RMS of interleaved s16le PCM, 0..1. */
export function pcmRms(data: Uint8Array): number {
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength)
  const samples = Math.floor(data.byteLength / 2)
  if (samples === 0) return 0
  let sum = 0
  for (let i = 0; i < samples; i++) {
    const s = view.getInt16(i * 2, true) / 32768
    sum += s * s
  }
  return Math.sqrt(sum / samples)
}

function containsWord(text: string | undefined, word: string): boolean {
  const target = word.toLowerCase()
  return (text ?? '')
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .includes(target)
}

function firstIndex(
  events: readonly SlowFinalEvent[],
  type: string,
  predicate: (e: SlowFinalEvent) => boolean = () => true,
): number {
  return events.findIndex((e) => e.type === type && predicate(e))
}

/** The `user_speaking_start` of a phrase: the first one at or after its first voiced frame. */
function speakingStartOf(events: readonly SlowFinalEvent[], phrase: PhraseRef): number {
  return firstIndex(events, SPEECH_EVENT_TYPE.userSpeakingStart, (e) => e.atMs >= phrase.sentAtMs)
}

/** The `user_speech_final` carrying the utterance id of the phrase's `user_speaking_start`. */
function finalOf(events: readonly SlowFinalEvent[], phrase: PhraseRef): number {
  const start = speakingStartOf(events, phrase)
  if (start < 0) return -1
  const id = events[start]!.utteranceId
  if (id == null) return -1
  return firstIndex(events, SPEECH_EVENT_TYPE.userSpeechFinal, (e) => e.utteranceId === id)
}

/**
 * Next turn on time: the phrase sent while an earlier final is still pending gets its
 * `user_speaking_start` within `maxMs` of its first voiced frame.
 */
export function checkNextTurnOnTime(params: {
  events: readonly SlowFinalEvent[]
  next: PhraseRef
  maxMs?: number
}): CheckResult {
  const maxMs = params.maxMs ?? NEXT_TURN_START_MAX_MS
  const idx = speakingStartOf(params.events, params.next)
  if (idx < 0) {
    return fail(`${params.next.label}: no user_speaking_start after its first voiced frame`)
  }
  const lateMs = params.events[idx]!.atMs - params.next.sentAtMs
  if (lateMs > maxMs) {
    return fail(
      `${params.next.label}: user_speaking_start ${lateMs} ms after its first voiced frame was sent, expected at most ${maxMs} ms (the inbound path waited for the pending final)`,
    )
  }
  return OK
}

/**
 * Finals arrive in utterance order: the first phrase's final (id of its `user_speaking_start`)
 * comes before the second phrase's. When a phrase has a `word`, its final must contain it.
 */
export function checkFinalOrder(params: {
  events: readonly SlowFinalEvent[]
  first: PhraseRef
  second: PhraseRef
}): CheckResult {
  const { events, first, second } = params
  const firstFinal = finalOf(events, first)
  const secondFinal = finalOf(events, second)
  if (firstFinal < 0) {
    return fail(
      `${first.label}: no user_speech_final with the utterance id of its user_speaking_start`,
    )
  }
  if (secondFinal < 0) {
    return fail(
      `${second.label}: no user_speech_final with the utterance id of its user_speaking_start`,
    )
  }
  for (const [phrase, idx] of [
    [first, firstFinal],
    [second, secondFinal],
  ] as const) {
    if (phrase.word != null && !containsWord(events[idx]!.text, phrase.word)) {
      return fail(
        `${phrase.label}: final ${JSON.stringify(events[idx]!.text ?? '')} does not contain "${phrase.word}"`,
      )
    }
  }
  if (firstFinal > secondFinal || events[firstFinal]!.atMs > events[secondFinal]!.atMs) {
    return fail(
      `${second.label}: final (+${events[secondFinal]!.atMs} ms) arrived before ${first.label}'s final (+${events[firstFinal]!.atMs} ms)`,
    )
  }
  return OK
}

/** No `user_stt_not_found` anywhere in the phase. */
export function checkNoSttNotFound(events: readonly SlowFinalEvent[]): CheckResult {
  const hit = events.find((e) => e.type === SPEECH_EVENT_TYPE.userSttNotFound)
  return hit == null ? OK : fail(`unexpected user_stt_not_found at +${hit.atMs} ms`)
}

/** At least `expected` `agent_speaking_start` events (the echo host answered each turn). */
export function checkRepliesStarted(params: {
  events: readonly SlowFinalEvent[]
  expected: number
}): CheckResult {
  const count = params.events.filter((e) => e.type === SPEECH_EVENT_TYPE.agentSpeakingStart).length
  return count >= params.expected
    ? OK
    : fail(`expected ${params.expected} agent_speaking_start, got ${count}`)
}

/**
 * VAD barge-in over a speaking agent: `barge_in` within `BARGE_MAX_MS` of the interrupting
 * phrase's first voiced frame, and `agent_speaking_end` within `SPEAKING_END_AFTER_BARGE_MAX_MS`
 * of `barge_in`.
 */
export function checkVadBargeOnTime(params: {
  events: readonly SlowFinalEvent[]
  interrupt: PhraseRef
  bargeMaxMs?: number
  speakingEndMaxMs?: number
}): CheckResult {
  const bargeMaxMs = params.bargeMaxMs ?? BARGE_MAX_MS
  const speakingEndMaxMs = params.speakingEndMaxMs ?? SPEAKING_END_AFTER_BARGE_MAX_MS
  const { events, interrupt } = params
  const bargeIdx = firstIndex(events, SPEECH_EVENT_TYPE.bargeIn)
  if (bargeIdx < 0) return fail(`no barge_in after ${interrupt.label} was sent over the agent`)
  const bargeAt = events[bargeIdx]!.atMs
  if (bargeAt < interrupt.sentAtMs) {
    return fail(
      `barge_in at +${bargeAt} ms came before ${interrupt.label} was sent (+${interrupt.sentAtMs} ms)`,
    )
  }
  const bargeLateMs = bargeAt - interrupt.sentAtMs
  if (bargeLateMs > bargeMaxMs) {
    return fail(
      `barge_in ${bargeLateMs} ms after ${interrupt.label}'s first voiced frame was sent, expected at most ${bargeMaxMs} ms`,
    )
  }
  const endIdx = firstIndex(events, SPEECH_EVENT_TYPE.agentSpeakingEnd, (e) => e.atMs >= bargeAt)
  if (endIdx < 0) return fail('no agent_speaking_end after barge_in')
  const endLateMs = events[endIdx]!.atMs - bargeAt
  if (endLateMs > speakingEndMaxMs) {
    return fail(
      `agent_speaking_end ${endLateMs} ms after barge_in, expected at most ${speakingEndMaxMs} ms`,
    )
  }
  return OK
}

/**
 * The speaker stops hearing the agent after barge-in: some audible frame while the agent spoke
 * (so the meter works), none in `[barge_in + 500 ms, barge_in + 1500 ms]`.
 */
export function checkAudioSilencedAfterBarge(params: {
  frames: readonly ReceivedFrame[]
  agentStartAtMs: number
  bargeAtMs: number
  audibleRms?: number
}): CheckResult {
  const audibleRms = params.audibleRms ?? AUDIBLE_RMS_THRESHOLD
  const heard = params.frames.some(
    (f) =>
      f.rms > audibleRms && f.atMs >= params.agentStartAtMs && f.atMs <= params.bargeAtMs + 100,
  )
  if (!heard) {
    return fail(
      `speaker received no audible frame (RMS > ${audibleRms}) between agent_speaking_start and barge_in; the audio meter saw nothing`,
    )
  }
  const from = params.bargeAtMs + QUIET_WINDOW_START_AFTER_BARGE_MS
  const to = params.bargeAtMs + QUIET_WINDOW_END_AFTER_BARGE_MS
  const loud = params.frames.find((f) => f.atMs >= from && f.atMs <= to && f.rms > audibleRms)
  if (loud != null) {
    return fail(
      `speaker still received audible agent audio (RMS ${loud.rms.toFixed(3)}) at +${loud.atMs} ms, ${loud.atMs - params.bargeAtMs} ms after barge_in; expected silence from +${QUIET_WINDOW_START_AFTER_BARGE_MS} ms to +${QUIET_WINDOW_END_AFTER_BARGE_MS} ms`,
    )
  }
  return OK
}

/**
 * Semantic barge-in (`requireSttPartial: true`) while an earlier final is pending:
 * - the later phrase's first `user_speech_partial` comes after the earlier phrase's
 *   `user_speech_final` (partials are attributed after the earlier final);
 * - `barge_in` comes after that partial;
 * - `barge_in` is at most `finalDelayMs + SEMANTIC_BARGE_SLACK_MS` after the later phrase's
 *   first voiced frame;
 * - the later phrase's final contains its `word`.
 */
export function checkSemanticBargeAfterEarlierFinal(params: {
  events: readonly SlowFinalEvent[]
  earlier: PhraseRef
  later: PhraseRef
  finalDelayMs?: number
}): CheckResult {
  const { events, earlier, later } = params
  const maxMs = (params.finalDelayMs ?? FINAL_DELAY_MS) + SEMANTIC_BARGE_SLACK_MS
  const earlierFinal = finalOf(events, earlier)
  if (earlierFinal < 0) {
    return fail(`${earlier.label}: no user_speech_final with the id of its user_speaking_start`)
  }
  const partial = firstIndex(
    events,
    SPEECH_EVENT_TYPE.userSpeechPartial,
    (e) => e.atMs >= later.sentAtMs,
  )
  if (partial < 0) return fail(`${later.label}: no user_speech_partial after it was sent`)
  if (partial < earlierFinal || events[partial]!.atMs < events[earlierFinal]!.atMs) {
    return fail(
      `${later.label}: first user_speech_partial (+${events[partial]!.atMs} ms) came before ${earlier.label}'s final (+${events[earlierFinal]!.atMs} ms)`,
    )
  }
  const barge = firstIndex(events, SPEECH_EVENT_TYPE.bargeIn, (e) => e.atMs >= later.sentAtMs)
  if (barge < 0) return fail(`no barge_in after ${later.label} was sent over the agent`)
  if (barge < partial) {
    return fail(
      `barge_in (+${events[barge]!.atMs} ms) came before ${later.label}'s first user_speech_partial (+${events[partial]!.atMs} ms)`,
    )
  }
  const bargeLateMs = events[barge]!.atMs - later.sentAtMs
  if (bargeLateMs > maxMs) {
    return fail(
      `barge_in ${bargeLateMs} ms after ${later.label}'s first voiced frame was sent, expected at most ${maxMs} ms`,
    )
  }
  const laterFinal = finalOf(events, later)
  if (laterFinal < 0) {
    return fail(`${later.label}: no user_speech_final with the id of its user_speaking_start`)
  }
  if (later.word != null && !containsWord(events[laterFinal]!.text, later.word)) {
    return fail(
      `${later.label}: final ${JSON.stringify(events[laterFinal]!.text ?? '')} does not contain "${later.word}"`,
    )
  }
  return OK
}

/** One line per event for failure output: `+1234ms user_speech_final id=… "text"`. */
export function formatTimeline(
  events: readonly SlowFinalEvent[],
  sends: readonly PhraseRef[] = [],
): string[] {
  const lines = [
    ...events.map((e) => ({
      atMs: e.atMs,
      line:
        `+${e.atMs}ms ${e.type}` +
        (e.utteranceId != null ? ` id=${e.utteranceId}` : '') +
        (e.text != null && e.text.length > 0 ? ` ${JSON.stringify(e.text)}` : ''),
    })),
    ...sends.map((s) => ({ atMs: s.sentAtMs, line: `+${s.sentAtMs}ms [speaker sent ${s.label}]` })),
  ]
  return lines.sort((a, b) => a.atMs - b.atMs).map((l) => l.line)
}
