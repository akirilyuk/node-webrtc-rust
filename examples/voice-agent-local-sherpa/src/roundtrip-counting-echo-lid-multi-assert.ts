/**
 * Assertions for 5-session concurrent echo + LID roundtrip — prefix + full one..ten order.
 */

import { SPEECH_EVENT_TYPE } from '@node-webrtc-rust/sdk/voice'

import {
  NUMBER_WORDS_ONE_TO_TEN,
  evaluateEchoLidInboundTranscript,
  transcriptHasPrefixBeforeCounting,
} from './roundtrip-counting-echo-lid-prefix.js'
import {
  evaluateUtteranceSessionCloseWithFinal,
  type LifecycleEvalResult,
  type LifecycleSpeechEvent,
} from './roundtrip-stt-lifecycle-helpers.js'

function normalizeForCompare(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9\s]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}

function countingWordsInOrder(text: string): string[] {
  const numberSet = new Set<string>(NUMBER_WORDS_ONE_TO_TEN)
  return normalizeForCompare(text)
    .split(' ')
    .filter((word) => word.length > 0 && numberSet.has(word))
}

/**
 * Inbound echo transcript must pass the staging prefix matcher and contain all ten
 * counting words in order (no clipped start, middle, or tail).
 */
export function assertFullCountingEcho(text: string): { ok: boolean; failures: string[] } {
  const failures: string[] = []
  const recognized = text.trim()

  if (!recognized) {
    return { ok: false, failures: ['inbound transcript is empty'] }
  }

  const prefixEval = evaluateEchoLidInboundTranscript({
    recognized,
    spokenCountingPhrase: NUMBER_WORDS_ONE_TO_TEN.join(' '),
    minNumberWords: 10,
  })
  failures.push(...prefixEval.failures)

  if (!transcriptHasPrefixBeforeCounting(recognized)) {
    failures.push(`missing any prefix before counting: "${recognized}"`)
  }

  const digits = countingWordsInOrder(recognized)

  for (let i = 0; i < NUMBER_WORDS_ONE_TO_TEN.length; i++) {
    const expected = NUMBER_WORDS_ONE_TO_TEN[i]!
    const got = digits[i]
    if (got !== expected) {
      failures.push(
        `counting word ${i + 1}: expected "${expected}", got "${got ?? 'missing'}" (full sequence: ${digits.join(' ') || '(none)'})`,
      )
      break
    }
  }

  if (digits.length < 10 && failures.every((f) => !f.startsWith('counting word'))) {
    failures.push(
      `expected all ten counting words in order, found ${digits.length}: ${digits.join(' ') || '(none)'}`,
    )
  }

  return { ok: failures.length === 0, failures }
}

/** Chunk 1 contract on echo host: user_language before final; close order; no TTS during LID. */
export function evaluateEchoLidEventOrdering(params: {
  events: LifecycleSpeechEvent[]
  label?: string
}): LifecycleEvalResult {
  const who = params.label ? `${params.label}: ` : ''
  const failures: string[] = []

  let finalIdx = -1
  for (let i = params.events.length - 1; i >= 0; i--) {
    if (params.events[i]!.type === SPEECH_EVENT_TYPE.userSpeechFinal) {
      finalIdx = i
      break
    }
  }
  if (finalIdx < 0) {
    return { passed: false, failures: [`${who}missing user_speech_final on echo side`] }
  }

  const throughFinal = params.events.slice(0, finalIdx + 1)
  let langIdx = -1
  for (let i = throughFinal.length - 1; i >= 0; i--) {
    if (throughFinal[i]!.type === SPEECH_EVENT_TYPE.userLanguage) {
      langIdx = i
      break
    }
  }
  if (langIdx < 0) {
    failures.push(`${who}missing user_language before user_speech_final`)
  } else if (langIdx > finalIdx) {
    failures.push(`${who}user_language must precede user_speech_final`)
  }

  const close = evaluateUtteranceSessionCloseWithFinal({
    events: throughFinal,
    label: params.label,
  })
  failures.push(...close.failures)

  const agentStartIdx = throughFinal.findIndex(
    (e) => e.type === SPEECH_EVENT_TYPE.agentSpeakingStart,
  )
  if (langIdx >= 0 && agentStartIdx >= 0 && langIdx >= agentStartIdx) {
    failures.push(
      `${who}user_language must precede first agent_speaking_start (no echo TTS while LID in flight)`,
    )
  }

  return { passed: failures.length === 0, failures }
}

/** Under 10-session load, catch-up can reach ~1.3; pre-fix double-write regression is ~1.87–2.0. */
const DEFAULT_ECHO_OUT_MS_MAX_RATIO = 1.2
const ECHO_OUT_MS_MAX_RATIO_UNDER_LOAD = 1.4

/** Echo reply outbound PCM must not exceed agent speaking wall time (mix pump double-write guard). */
export function computeEchoOutMsRatio(params: {
  outMs: number
  agentSpeakingStartAt: number | null
  agentSpeakingEndAt: number | null
}): number | null {
  if (params.agentSpeakingStartAt == null || params.agentSpeakingEndAt == null) {
    return null
  }
  const wallMs = params.agentSpeakingEndAt - params.agentSpeakingStartAt
  if (wallMs <= 0) {
    return null
  }
  return params.outMs / wallMs
}

export function assertEchoOutboundPcmWithinSpeakingWindow(params: {
  sessionId?: string
  outMs: number
  agentSpeakingStartAt: number | null
  agentSpeakingEndAt: number | null
  maxRatio?: number
}): { ok: boolean; outMsRatio: number | null; failures: string[] } {
  const prefix = params.sessionId ? `${params.sessionId} echo outbound: ` : ''
  const maxRatio = params.maxRatio ?? ECHO_OUT_MS_MAX_RATIO_UNDER_LOAD
  const displayMax = DEFAULT_ECHO_OUT_MS_MAX_RATIO
  const outMsRatio = computeEchoOutMsRatio(params)
  if (outMsRatio == null) {
    return { ok: true, outMsRatio: null, failures: [] }
  }
  const wallMs = params.agentSpeakingEndAt! - params.agentSpeakingStartAt!
  const ratioBp = Math.round(outMsRatio * 100)
  const maxBp = Math.round(maxRatio * 100)
  if (ratioBp > maxBp) {
    return {
      ok: false,
      outMsRatio,
      failures: [
        `${prefix}mix pump wrote more PCM than real time — double write? outMs=${params.outMs} wallMs=${wallMs} ratio=${outMsRatio.toFixed(2)} (max ${displayMax})`,
      ],
    }
  }
  return { ok: true, outMsRatio, failures: [] }
}
