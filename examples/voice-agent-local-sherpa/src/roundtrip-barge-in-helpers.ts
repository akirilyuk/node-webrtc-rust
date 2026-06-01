/**
 * Evaluators for semantic barge-in roundtrip (STT partial → barge_in → agent_speaking_end).
 * Unit-tested without Sherpa; used by roundtrip-barge-in.ts E2E.
 */

import type { SpeechEvent, SpeechEventType } from '@node-webrtc-rust/sdk/voice'

export interface RecordedSpeechEvent {
  type: SpeechEventType
  atMs: number
  text?: string
}

/** Default max gap partial → barge_in (regression for SpeechEnd clearing await before poll). */
export const DEFAULT_MAX_PARTIAL_TO_BARGE_MS = 500

/** Default max gap barge_in → agent_speaking_end after flush. */
export const DEFAULT_MAX_BARGE_TO_AGENT_END_MS = 2000

export function recordSpeechEvent(
  events: RecordedSpeechEvent[],
  event: SpeechEvent,
  startedAtMs: number,
): void {
  events.push({
    type: event.type,
    atMs: Date.now() - startedAtMs,
    text: event.text,
  })
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
    if (e.type !== 'user_speech_partial') continue
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

  const agentStartIdx = indexOfType(params.events, 'agent_speaking_start')
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

  const bargeIdx = indexOfType(params.events.slice(agentStartIdx + 1), 'barge_in')
  const bargeAbsIdx = bargeIdx >= 0 ? agentStartIdx + 1 + bargeIdx : -1

  const agentEndIdx = lastIndexOfType(params.events, 'agent_speaking_end')
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
    (e, i) => i < partialIdx && e.type === 'barge_in' && i > agentStartIdx,
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

export function evaluateToneMustNotBarge(params: {
  events: RecordedSpeechEvent[]
  bargeCount: number
}): { passed: boolean; failures: string[] } {
  const failures: string[] = []
  if (params.bargeCount > 0) {
    failures.push(`tone phase must not emit barge_in (saw ${params.bargeCount})`)
  }
  const partialBeforeBarge = params.events.some((e) => e.type === 'user_speech_partial')
  const barge = params.events.some((e) => e.type === 'barge_in')
  if (partialBeforeBarge && barge) {
    failures.push('tone phase must not barge even if a stray partial appears')
  }
  return { passed: failures.length === 0, failures }
}
