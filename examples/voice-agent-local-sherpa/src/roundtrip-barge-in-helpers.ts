/**
 * Evaluators for semantic barge-in roundtrip (STT partial → barge_in → agent_speaking_end).
 * Unit-tested without Sherpa; used by roundtrip-barge-in.ts E2E.
 */

import {
  SPEECH_EVENT_TYPE,
  type SpeechEvent,
  type SpeechEventType,
} from '@node-webrtc-rust/sdk/voice'

import { DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS, wordSimilarity } from './roundtrip-counting.js'

export interface RecordedSpeechEvent {
  type: SpeechEventType
  atMs: number
  text?: string
}

/** Default max gap partial → barge_in (regression for SpeechEnd clearing await before poll). */
export const DEFAULT_MAX_PARTIAL_TO_BARGE_MS = 500

/** Default max gap barge_in → agent_speaking_end after flush. */
export const DEFAULT_MAX_BARGE_TO_AGENT_END_MS = 2000

/** Default min word similarity for barge phrase `user_speech_final`. */
export const DEFAULT_BARGE_PHRASE_MIN_SIMILARITY = 0.6

export function recordSpeechEvent(
  events: RecordedSpeechEvent[],
  event: SpeechEvent,
  startedAtMs: number,
): RecordedSpeechEvent {
  const recorded: RecordedSpeechEvent = {
    type: event.type,
    atMs: Date.now() - startedAtMs,
    text: event.text,
  }
  events.push(recorded)
  return recorded
}

/** One-line event for E2E logs: `+1234ms user_speech_partial "stop now"`. */
export function formatRecordedSpeechEvent(event: RecordedSpeechEvent): string {
  const text = event.text != null && event.text.length > 0 ? ` ${JSON.stringify(event.text)}` : ''
  return `+${event.atMs}ms ${event.type}${text}`
}

/** Phase 2 done when agent TTS finishes without barge. */
export function phase2EventsComplete(events: RecordedSpeechEvent[]): boolean {
  const agentStart = events.findIndex((e) => e.type === SPEECH_EVENT_TYPE.agentSpeakingStart)
  if (agentStart < 0) return false
  return events.slice(agentStart + 1).some((e) => e.type === SPEECH_EVENT_TYPE.agentSpeakingEnd)
}

function sliceAfterBargeInPhase3(events: RecordedSpeechEvent[]): RecordedSpeechEvent[] | null {
  const agentStart = events.findIndex((e) => e.type === SPEECH_EVENT_TYPE.agentSpeakingStart)
  if (agentStart < 0) return null
  const afterStart = events.slice(agentStart + 1)
  const bargeIdx = afterStart.findIndex((e) => e.type === SPEECH_EVENT_TYPE.bargeIn)
  if (bargeIdx < 0) return null
  return afterStart.slice(bargeIdx + 1)
}

/** Phase 3 mid-run: barge-in fired and agent TTS stopped (finalize may still be pending). */
export function phase3BargeObserved(events: RecordedSpeechEvent[]): boolean {
  const agentStart = events.findIndex((e) => e.type === SPEECH_EVENT_TYPE.agentSpeakingStart)
  if (agentStart < 0) return false
  const afterStart = events.slice(agentStart + 1)
  return (
    afterStart.some((e) => e.type === SPEECH_EVENT_TYPE.bargeIn) &&
    afterStart.some((e) => e.type === SPEECH_EVENT_TYPE.agentSpeakingEnd)
  )
}

export function hasUserSpeechFinal(events: RecordedSpeechEvent[]): boolean {
  return events.some((e) => e.type === SPEECH_EVENT_TYPE.userSpeechFinal)
}

/** Phase 3 done when barge truncated TTS and listener finalized the barge utterance. */
export function phase3EventsComplete(events: RecordedSpeechEvent[]): boolean {
  const afterBarge = sliceAfterBargeInPhase3(events)
  if (afterBarge == null) return false
  const agentEnded = afterBarge.some((e) => e.type === SPEECH_EVENT_TYPE.agentSpeakingEnd)
  const hasFinal = afterBarge.some((e) => e.type === SPEECH_EVENT_TYPE.userSpeechFinal)
  return agentEnded && hasFinal
}

function qualifyingPartialAfter(
  events: RecordedSpeechEvent[],
  agentStart: number,
  minChars = 2,
): boolean {
  for (let i = agentStart + 1; i < events.length; i++) {
    const e = events[i]!
    if (e.type !== SPEECH_EVENT_TYPE.userSpeechPartial) continue
    if ((e.text?.trim().length ?? 0) >= minChars) return true
  }
  return false
}

/**
 * Phase 3 collection stop: success, or fail-fast when the outcome is clear.
 * Keeps waiting after agent_speaking_end if the user spoke but STT partial has not arrived yet.
 */
export function phase3EventsTerminal(events: RecordedSpeechEvent[]): boolean {
  if (phase3EventsComplete(events)) return true
  const agentStart = events.findIndex((e) => e.type === SPEECH_EVENT_TYPE.agentSpeakingStart)
  if (agentStart < 0) return false
  const afterStart = events.slice(agentStart + 1)
  const hadBarge = afterStart.some((e) => e.type === SPEECH_EVENT_TYPE.bargeIn)
  const userSpoke = afterStart.some((e) => e.type === SPEECH_EVENT_TYPE.userSpeakingStart)
  const agentEnded = afterStart.some((e) => e.type === SPEECH_EVENT_TYPE.agentSpeakingEnd)
  const hasPartial = qualifyingPartialAfter(events, agentStart)

  // Barge path: keep collecting until user_speech_final (or wall-clock cap in harness).
  if (
    hadBarge &&
    agentEnded &&
    !afterStart.some((e) => e.type === SPEECH_EVENT_TYPE.userSpeechFinal)
  ) {
    return false
  }

  if (agentEnded && !userSpoke && !hadBarge) return true
  if (agentEnded && userSpoke && hasPartial && !hadBarge) return true
  // User-leg audio finished agent TTS without barge — outcome is final (pass or fail).
  if (agentEnded && userSpoke && !hadBarge) return true
  return false
}

/** Phase 1 baseline: full agent playback from start through end. */
export function phase1BaselineComplete(events: RecordedSpeechEvent[]): boolean {
  return phase2EventsComplete(events)
}

/** Print the full event timeline for a phase (empty phases still log a header). */
export function logRecordedSpeechEvents(events: RecordedSpeechEvent[], label: string): void {
  console.log(`[${label}] event timeline (${events.length} events):`)
  if (events.length === 0) {
    console.log(`[${label}]   (none)`)
    return
  }
  for (const event of events) {
    console.log(`[${label}]   ${formatRecordedSpeechEvent(event)}`)
  }
}

function indexOfType(events: RecordedSpeechEvent[], type: SpeechEventType): number {
  return events.findIndex((e) => e.type === type)
}

function lastIndexOfType(events: RecordedSpeechEvent[], type: SpeechEventType): number {
  for (let i = events.length - 1; i >= 0; i--) {
    if (events[i]!.type === type) return i
  }
  return -1
}

function qualifyingPartialIndex(
  events: RecordedSpeechEvent[],
  afterIndex: number,
  minChars: number,
): number {
  for (let i = afterIndex + 1; i < events.length; i++) {
    const e = events[i]!
    if (e.type !== SPEECH_EVENT_TYPE.userSpeechPartial) continue
    const t = e.text?.trim() ?? ''
    if (t.length >= minChars) return i
  }
  return -1
}

export interface SemanticBargeOrderResult {
  passed: boolean
  failures: string[]
  agentStartAtMs: number | null
  partialAtMs: number | null
  bargeAtMs: number | null
  agentEndAtMs: number | null
}

/**
 * After agent TTS has started, the first qualifying partial must precede barge_in,
 * and agent_speaking_end must follow barge_in (semantic interrupt path).
 */
export function evaluateSemanticBargeEventOrder(params: {
  events: RecordedSpeechEvent[]
  minPartialChars?: number
  maxPartialToBargeMs?: number
  maxBargeToAgentEndMs?: number
  label?: string
}): SemanticBargeOrderResult {
  const who = params.label ? `${params.label}: ` : ''
  const minChars = params.minPartialChars ?? 2
  const maxPartialToBarge = params.maxPartialToBargeMs ?? DEFAULT_MAX_PARTIAL_TO_BARGE_MS
  const maxBargeToEnd = params.maxBargeToAgentEndMs ?? DEFAULT_MAX_BARGE_TO_AGENT_END_MS
  const failures: string[] = []

  const agentStartIdx = indexOfType(params.events, SPEECH_EVENT_TYPE.agentSpeakingStart)
  if (agentStartIdx < 0) {
    failures.push(`${who}missing agent_speaking_start in event stream`)
    return {
      passed: false,
      failures,
      agentStartAtMs: null,
      partialAtMs: null,
      bargeAtMs: null,
      agentEndAtMs: null,
    }
  }

  const partialIdx = qualifyingPartialIndex(params.events, agentStartIdx, minChars)
  if (partialIdx < 0) {
    failures.push(`${who}missing qualifying user_speech_partial after agent_speaking_start`)
  }

  const bargeIdx = indexOfType(params.events.slice(agentStartIdx + 1), SPEECH_EVENT_TYPE.bargeIn)
  const bargeAbsIdx = bargeIdx >= 0 ? agentStartIdx + 1 + bargeIdx : -1

  const agentEndIdx = lastIndexOfType(params.events, SPEECH_EVENT_TYPE.agentSpeakingEnd)
  const agentEndAfterStart = agentEndIdx > agentStartIdx

  const agentStartAtMs = params.events[agentStartIdx]!.atMs
  const partialAtMs = partialIdx >= 0 ? params.events[partialIdx]!.atMs : null
  const bargeAtMs = bargeAbsIdx >= 0 ? params.events[bargeAbsIdx]!.atMs : null
  const agentEndAtMs =
    agentEndAfterStart && agentEndIdx >= 0 ? params.events[agentEndIdx]!.atMs : null

  if (bargeAbsIdx < 0) {
    failures.push(`${who}missing barge_in after agent_speaking_start`)
  }

  if (partialIdx >= 0 && bargeAbsIdx >= 0 && bargeAbsIdx < partialIdx) {
    failures.push(`${who}barge_in must not precede qualifying user_speech_partial`)
  }

  if (partialIdx >= 0 && bargeAbsIdx >= 0) {
    const gap = params.events[bargeAbsIdx]!.atMs - params.events[partialIdx]!.atMs
    if (gap > maxPartialToBarge) {
      failures.push(`${who}partial → barge_in gap ${gap} ms exceeds ${maxPartialToBarge} ms`)
    }
  }

  if (bargeAbsIdx >= 0 && agentEndAfterStart) {
    if (agentEndIdx <= bargeAbsIdx) {
      failures.push(`${who}agent_speaking_end must follow barge_in`)
    } else {
      const gap = params.events[agentEndIdx]!.atMs - params.events[bargeAbsIdx]!.atMs
      if (gap > maxBargeToEnd) {
        failures.push(
          `${who}barge_in → agent_speaking_end gap ${gap} ms exceeds ${maxBargeToEnd} ms`,
        )
      }
    }
  } else if (bargeAbsIdx >= 0) {
    failures.push(`${who}missing agent_speaking_end after barge_in`)
  }

  const earlyBarge = params.events.findIndex(
    (e, i) => i < partialIdx && e.type === SPEECH_EVENT_TYPE.bargeIn && i > agentStartIdx,
  )
  if (partialIdx >= 0 && earlyBarge >= 0) {
    failures.push(
      `${who}barge_in at ${params.events[earlyBarge]!.atMs} ms before partial (instant VAD path during agent TTS)`,
    )
  }

  return {
    passed: failures.length === 0,
    failures,
    agentStartAtMs,
    partialAtMs,
    bargeAtMs,
    agentEndAtMs,
  }
}

export interface BargeUtteranceFinalResult {
  passed: boolean
  failures: string[]
  recognized: string
  similarity: number
  speakingEndAtMs: number | null
  finalAtMs: number | null
  endToFinalGapMs: number | null
}

/**
 * After semantic barge: listener must emit user_speaking_end → user_speech_final
 * with text matching the barge phrase (Sherpa recognized the interrupt).
 */
export function evaluateBargeUtteranceFinal(params: {
  events: RecordedSpeechEvent[]
  expectedPhrase: string
  minSimilarity?: number
  maxEndToFinalMs?: number
  label?: string
}): BargeUtteranceFinalResult {
  const who = params.label ? `${params.label}: ` : ''
  const minSim = params.minSimilarity ?? DEFAULT_BARGE_PHRASE_MIN_SIMILARITY
  const maxGap = params.maxEndToFinalMs ?? DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS
  const failures: string[] = []

  const afterBarge = sliceAfterBargeInPhase3(params.events)
  if (afterBarge == null) {
    failures.push(`${who}missing barge_in after agent_speaking_start`)
    return {
      passed: false,
      failures,
      recognized: '',
      similarity: 0,
      speakingEndAtMs: null,
      finalAtMs: null,
      endToFinalGapMs: null,
    }
  }

  const finals = afterBarge.filter((e) => e.type === SPEECH_EVENT_TYPE.userSpeechFinal)
  const ends = afterBarge.filter((e) => e.type === SPEECH_EVENT_TYPE.userSpeakingEnd)

  if (finals.length === 0) {
    failures.push(`${who}missing user_speech_final after barge_in`)
  } else if (finals.length > 1) {
    failures.push(`${who}expected 1 user_speech_final after barge_in, got ${finals.length}`)
  }
  if (ends.length === 0) {
    failures.push(`${who}missing user_speaking_end after barge_in`)
  } else if (ends.length > 1) {
    failures.push(`${who}expected 1 user_speaking_end after barge_in, got ${ends.length}`)
  }

  const final = finals[0]
  const end = ends[0]
  const recognized = final?.text?.trim() ?? ''
  const similarity = recognized ? wordSimilarity(params.expectedPhrase, recognized) : 0
  const speakingEndAtMs = end?.atMs ?? null
  const finalAtMs = final?.atMs ?? null
  let endToFinalGapMs: number | null = null

  if (!recognized) {
    failures.push(`${who}user_speech_final text is empty after barge_in`)
  } else if (similarity < minSim) {
    failures.push(
      `${who}barge phrase similarity ${(similarity * 100).toFixed(0)}% < ${(minSim * 100).toFixed(0)}% (expected "${params.expectedPhrase}", got "${recognized}")`,
    )
  }

  if (speakingEndAtMs != null && finalAtMs != null) {
    endToFinalGapMs = finalAtMs - speakingEndAtMs
    if (endToFinalGapMs < 0) {
      failures.push(`${who}user_speech_final before user_speaking_end (${-endToFinalGapMs} ms)`)
    } else if (endToFinalGapMs > maxGap) {
      failures.push(
        `${who}user_speaking_end → user_speech_final gap ${endToFinalGapMs} ms exceeds ${maxGap} ms`,
      )
    }
  }

  return {
    passed: failures.length === 0,
    failures,
    recognized,
    similarity,
    speakingEndAtMs,
    finalAtMs,
    endToFinalGapMs,
  }
}

export function evaluateToneMustNotBarge(params: {
  events: RecordedSpeechEvent[]
  bargeCount: number
}): { passed: boolean; failures: string[] } {
  const failures: string[] = []
  if (params.bargeCount > 0) {
    failures.push(`tone phase must not emit barge_in (saw ${params.bargeCount})`)
  }
  const partialBeforeBarge = params.events.some(
    (e) => e.type === SPEECH_EVENT_TYPE.userSpeechPartial,
  )
  const barge = params.events.some((e) => e.type === SPEECH_EVENT_TYPE.bargeIn)
  if (partialBeforeBarge && barge) {
    failures.push('tone phase must not barge even if a stray partial appears')
  }
  return { passed: failures.length === 0, failures }
}
