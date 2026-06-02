/**
 * Shared evaluators for VAD/STT utterance lifecycle events (vad_triggered, stt_stream_*,
 * user_stt_*). Unit-tested without Sherpa; used by roundtrip E2E harnesses.
 */

import { SPEECH_EVENT_TYPE, type SpeechEventType } from '@node-webrtc-rust/sdk/voice'

export interface LifecycleSpeechEvent {
  type: SpeechEventType
  atMs: number
  text?: string
}

export interface LifecycleEvalResult {
  passed: boolean
  failures: string[]
}

/** Default max gap vad_triggered → stt_stream_start (same SpeechStart frame). */
export const DEFAULT_MAX_VAD_TO_STT_STREAM_MS = 100

/** Default max gap stt_stream_start → first qualifying partial during agent TTS. */
export const DEFAULT_MAX_STT_STREAM_TO_PARTIAL_MS = 3000

function indexOfType(events: LifecycleSpeechEvent[], type: SpeechEventType): number {
  return events.findIndex((e) => e.type === type)
}

function lastIndexOfType(events: LifecycleSpeechEvent[], type: SpeechEventType): number {
  for (let i = events.length - 1; i >= 0; i--) {
    if (events[i]!.type === type) return i
  }
  return -1
}

function qualifyingPartialIndex(
  events: LifecycleSpeechEvent[],
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

function prefix(who: string | undefined): string {
  return who ? `${who}: ` : ''
}

/**
 * Each VAD SpeechStart opens STT: vad_triggered → user_stt_start → stt_stream_start.
 * When `expectCount` is set, requires that many open sequences (e.g. two-phrase roundtrip).
 */
export function evaluateVadSttSessionOpen(params: {
  events: LifecycleSpeechEvent[]
  expectCount?: number
  maxVadToSttStreamMs?: number
  label?: string
}): LifecycleEvalResult {
  const who = prefix(params.label)
  const failures: string[] = []
  const maxVadGap = params.maxVadToSttStreamMs ?? DEFAULT_MAX_VAD_TO_STT_STREAM_MS
  const expectCount = params.expectCount ?? 1

  const vadIndices = params.events
    .map((e, i) => (e.type === SPEECH_EVENT_TYPE.vadTriggered ? i : -1))
    .filter((i) => i >= 0)

  if (vadIndices.length < expectCount) {
    failures.push(`${who}expected at least ${expectCount} vad_triggered, got ${vadIndices.length}`)
  }

  for (let n = 0; n < Math.min(expectCount, vadIndices.length); n++) {
    const vadIdx = vadIndices[n]!
    const slice = params.events.slice(vadIdx)
    const userSttIdx = slice.findIndex((e) => e.type === SPEECH_EVENT_TYPE.userSttStart)
    const sttIdx = slice.findIndex((e) => e.type === SPEECH_EVENT_TYPE.sttStreamStart)
    const tag = expectCount > 1 ? ` (utterance ${n + 1})` : ''

    if (userSttIdx < 0) {
      failures.push(`${who}missing user_stt_start after vad_triggered${tag}`)
    }
    if (sttIdx < 0) {
      failures.push(`${who}missing stt_stream_start after vad_triggered${tag}`)
    }
    if (userSttIdx >= 0 && sttIdx >= 0 && sttIdx < userSttIdx) {
      failures.push(`${who}stt_stream_start must follow user_stt_start${tag}`)
    }
    if (userSttIdx >= 0 && sttIdx >= 0) {
      const gap = params.events[vadIdx + sttIdx]!.atMs - params.events[vadIdx]!.atMs
      if (gap > maxVadGap) {
        failures.push(
          `${who}vad_triggered → stt_stream_start gap ${gap} ms exceeds ${maxVadGap} ms${tag}`,
        )
      }
    }
  }

  return { passed: failures.length === 0, failures }
}

/**
 * Successful utterance close: stt_stream_end → user_stt_end → user_speaking_end → user_speech_final.
 */
export function evaluateUtteranceSessionCloseWithFinal(params: {
  events: LifecycleSpeechEvent[]
  label?: string
}): LifecycleEvalResult {
  const who = prefix(params.label)
  const failures: string[] = []
  const finalIdx = lastIndexOfType(params.events, SPEECH_EVENT_TYPE.userSpeechFinal)
  if (finalIdx < 0) {
    return { passed: false, failures: [`${who}missing user_speech_final for session close check`] }
  }

  const beforeFinal = params.events.slice(0, finalIdx + 1)
  const streamEndIdx = lastIndexOfType(beforeFinal, SPEECH_EVENT_TYPE.sttStreamEnd)
  const userSttEndIdx = lastIndexOfType(beforeFinal, SPEECH_EVENT_TYPE.userSttEnd)
  const speakingEndIdx = lastIndexOfType(beforeFinal, SPEECH_EVENT_TYPE.userSpeakingEnd)

  if (streamEndIdx < 0) {
    failures.push(`${who}missing stt_stream_end before user_speech_final`)
  }
  if (userSttEndIdx < 0) {
    failures.push(`${who}missing user_stt_end before user_speech_final`)
  }
  if (speakingEndIdx < 0) {
    failures.push(`${who}missing user_speaking_end before user_speech_final`)
  }

  if (streamEndIdx >= 0 && userSttEndIdx >= 0 && userSttEndIdx < streamEndIdx) {
    failures.push(`${who}user_stt_end must follow stt_stream_end`)
  }
  if (userSttEndIdx >= 0 && speakingEndIdx >= 0 && speakingEndIdx < userSttEndIdx) {
    failures.push(`${who}user_speaking_end must follow user_stt_end`)
  }
  if (speakingEndIdx >= 0 && finalIdx >= 0 && finalIdx < speakingEndIdx) {
    failures.push(`${who}user_speech_final must follow user_speaking_end`)
  }

  return { passed: failures.length === 0, failures }
}

/**
 * C1 path: VAD fired but no STT transcript — stt_stream_end → user_stt_not_found → user_stt_end, no final.
 */
export function evaluateC1NotFoundPath(params: {
  events: LifecycleSpeechEvent[]
  label?: string
}): LifecycleEvalResult {
  const who = prefix(params.label)
  const failures: string[] = []
  const notFoundIdx = indexOfType(params.events, SPEECH_EVENT_TYPE.userSttNotFound)
  if (notFoundIdx < 0) {
    return { passed: false, failures: [`${who}missing user_stt_not_found`] }
  }

  const beforeNotFound = params.events.slice(0, notFoundIdx)
  const afterNotFound = params.events.slice(notFoundIdx)
  const streamEndIdx = lastIndexOfType(beforeNotFound, SPEECH_EVENT_TYPE.sttStreamEnd)
  const userSttEndIdx = afterNotFound.findIndex((e) => e.type === SPEECH_EVENT_TYPE.userSttEnd)
  const finalIdx = indexOfType(params.events, SPEECH_EVENT_TYPE.userSpeechFinal)

  if (streamEndIdx < 0) {
    failures.push(`${who}missing stt_stream_end before user_stt_not_found`)
  }
  if (userSttEndIdx < 0) {
    failures.push(`${who}missing user_stt_end after user_stt_not_found`)
  }
  if (finalIdx >= 0) {
    failures.push(`${who}user_speech_final must not accompany user_stt_not_found (C1)`)
  }

  const partialBeforeNotFound = beforeNotFound.filter(
    (e) => e.type === SPEECH_EVENT_TYPE.userSpeechPartial,
  ).length
  if (partialBeforeNotFound > 0) {
    failures.push(`${who}user_stt_not_found must not follow user_speech_partial (C1)`)
  }

  return { passed: failures.length === 0, failures }
}

/** Any utterance with ≥1 partial must eventually get user_speech_final (normal or C2 forced). */
export function evaluateNoPartialWithoutFinal(params: {
  events: LifecycleSpeechEvent[]
  label?: string
}): LifecycleEvalResult {
  const who = prefix(params.label)
  const failures: string[] = []
  const partialCount = params.events.filter(
    (e) => e.type === SPEECH_EVENT_TYPE.userSpeechPartial,
  ).length
  const finalCount = params.events.filter(
    (e) => e.type === SPEECH_EVENT_TYPE.userSpeechFinal,
  ).length
  const notFoundCount = params.events.filter(
    (e) => e.type === SPEECH_EVENT_TYPE.userSttNotFound,
  ).length
  if (partialCount > 0 && finalCount === 0 && notFoundCount === 0) {
    failures.push(
      `${who}orphan partial(s): ${partialCount} user_speech_partial without user_speech_final`,
    )
  }
  return { passed: failures.length === 0, failures }
}

/**
 * After agent TTS starts, barge listen: vad_triggered → user_stt_start → stt_stream_start
 * before qualifying partial; partial must not precede stt_stream_start.
 */
export function evaluateSttLifecycleOnBargePath(params: {
  events: LifecycleSpeechEvent[]
  maxVadToSttStreamMs?: number
  maxSttStreamToPartialMs?: number
  minPartialChars?: number
  label?: string
}): LifecycleEvalResult {
  const who = prefix(params.label)
  const failures: string[] = []
  const minChars = params.minPartialChars ?? 2
  const agentStartIdx = indexOfType(params.events, SPEECH_EVENT_TYPE.agentSpeakingStart)
  if (agentStartIdx < 0) {
    return { passed: false, failures: [`${who}missing agent_speaking_start`] }
  }
  const afterStart = params.events.slice(agentStartIdx + 1)
  const vadIdx = afterStart.findIndex((e) => e.type === SPEECH_EVENT_TYPE.vadTriggered)
  const userSttIdx = afterStart.findIndex((e) => e.type === SPEECH_EVENT_TYPE.userSttStart)
  const sttIdx = afterStart.findIndex((e) => e.type === SPEECH_EVENT_TYPE.sttStreamStart)
  const partialIdx = qualifyingPartialIndex(params.events, agentStartIdx, minChars)

  if (vadIdx < 0) failures.push(`${who}missing vad_triggered after agent_speaking_start`)
  if (userSttIdx < 0) failures.push(`${who}missing user_stt_start after agent_speaking_start`)
  if (sttIdx < 0) failures.push(`${who}missing stt_stream_start after agent_speaking_start`)

  if (vadIdx >= 0 && userSttIdx >= 0 && userSttIdx < vadIdx) {
    failures.push(`${who}user_stt_start must follow vad_triggered`)
  }
  if (userSttIdx >= 0 && sttIdx >= 0 && sttIdx < userSttIdx) {
    failures.push(`${who}stt_stream_start must follow user_stt_start`)
  }
  if (partialIdx >= 0 && sttIdx >= 0) {
    const sttAbs = agentStartIdx + 1 + sttIdx
    if (partialIdx < sttAbs) {
      failures.push(`${who}user_speech_partial must not precede stt_stream_start on barge path`)
    }
    const maxGap = params.maxSttStreamToPartialMs ?? DEFAULT_MAX_STT_STREAM_TO_PARTIAL_MS
    const gap = params.events[partialIdx]!.atMs - params.events[sttAbs]!.atMs
    if (gap > maxGap) {
      failures.push(`${who}stt_stream_start → partial gap ${gap} ms exceeds ${maxGap} ms`)
    }
  }
  if (vadIdx >= 0 && sttIdx >= 0) {
    const maxVadGap = params.maxVadToSttStreamMs ?? DEFAULT_MAX_VAD_TO_STT_STREAM_MS
    const vadAbs = agentStartIdx + 1 + vadIdx
    const sttAbs = agentStartIdx + 1 + sttIdx
    const gap = params.events[sttAbs]!.atMs - params.events[vadAbs]!.atMs
    if (gap > maxVadGap) {
      failures.push(`${who}vad_triggered → stt_stream_start gap ${gap} ms exceeds ${maxVadGap} ms`)
    }
  }
  return { passed: failures.length === 0, failures }
}

/** Normal listen path (counting, two-phrase, utterance-timing): open + close + no orphan partials. */
export function evaluateNormalUtteranceLifecycle(params: {
  events: LifecycleSpeechEvent[]
  expectOpenCount?: number
  label?: string
}): LifecycleEvalResult {
  const open = evaluateVadSttSessionOpen({
    events: params.events,
    expectCount: params.expectOpenCount ?? 1,
    label: params.label,
  })
  const close = evaluateUtteranceSessionCloseWithFinal({
    events: params.events,
    label: params.label,
  })
  const orphans = evaluateNoPartialWithoutFinal({
    events: params.events,
    label: params.label,
  })
  const failures = [...open.failures, ...close.failures, ...orphans.failures]
  return { passed: failures.length === 0, failures }
}

/** Semantic barge Phase 3: barge-path open + session close + no orphan partials. */
export function evaluateBargePathLifecycle(params: {
  events: LifecycleSpeechEvent[]
  label?: string
}): LifecycleEvalResult {
  const bargeOpen = evaluateSttLifecycleOnBargePath({
    events: params.events,
    label: params.label,
  })
  const close = evaluateUtteranceSessionCloseWithFinal({
    events: params.events,
    label: params.label,
  })
  const orphans = evaluateNoPartialWithoutFinal({
    events: params.events,
    label: params.label,
  })
  const failures = [...bargeOpen.failures, ...close.failures, ...orphans.failures]
  return { passed: failures.length === 0, failures }
}

/** Tone / no-transcript phase: no barge, no final; optional C1 not_found if STT session opened. */
export function evaluateTonePhaseLifecycle(params: {
  events: LifecycleSpeechEvent[]
  bargeCount: number
  label?: string
}): LifecycleEvalResult {
  const who = prefix(params.label)
  const failures: string[] = []
  if (params.bargeCount > 0) {
    failures.push(`${who}tone phase must not emit barge_in (saw ${params.bargeCount})`)
  }
  const finalCount = params.events.filter(
    (e) => e.type === SPEECH_EVENT_TYPE.userSpeechFinal,
  ).length
  if (finalCount > 0) {
    failures.push(`${who}tone phase must not emit user_speech_final (saw ${finalCount})`)
  }
  const partialCount = params.events.filter(
    (e) => e.type === SPEECH_EVENT_TYPE.userSpeechPartial,
  ).length
  if (partialCount > 0 && params.bargeCount > 0) {
    failures.push(`${who}tone phase must not barge even if a stray partial appears`)
  }

  const notFoundIdx = indexOfType(params.events, SPEECH_EVENT_TYPE.userSttNotFound)
  const sttOpenIdx = indexOfType(params.events, SPEECH_EVENT_TYPE.sttStreamStart)
  if (notFoundIdx >= 0) {
    const c1 = evaluateC1NotFoundPath({ events: params.events, label: params.label })
    failures.push(...c1.failures)
  } else if (sttOpenIdx >= 0 && partialCount === 0 && finalCount === 0) {
    // STT opened on VAD but C1 may not have fired yet — at least verify open sequence.
    const open = evaluateVadSttSessionOpen({ events: params.events, label: params.label })
    failures.push(...open.failures)
  }

  return { passed: failures.length === 0, failures }
}

/** Merge multiple lifecycle eval results into one. */
export function mergeLifecycleResults(...results: LifecycleEvalResult[]): LifecycleEvalResult {
  const failures = results.flatMap((r) => r.failures)
  return { passed: failures.length === 0, failures }
}
