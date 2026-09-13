/**
 * Env knobs for echo + LID roundtrip harnesses (ttsExclusion, session count).
 */

import type { VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

import { CLOUD_DEFAULT_LID_MIN_SPEECH_MS } from './roundtrip-counting-echo-lid-prefix.js'

type EchoLanguageIdConfig = NonNullable<VoiceAgentConfig['languageId']>

export const DEFAULT_MULTI_SESSIONS = 10
export const MAX_MULTI_SESSIONS = 20

/** `0`/`false`/`off` → false; `1`/`true`/`on` → true; unset/junk → undefined (library default). */
export function parseSherpaLidTtsExclusion(
  env: NodeJS.ProcessEnv = process.env,
): boolean | undefined {
  const raw = env.SHERPA_LID_TTS_EXCLUSION?.trim()
  if (!raw) {
    return undefined
  }
  const lower = raw.toLowerCase()
  if (raw === '1' || lower === 'true' || lower === 'on') {
    return true
  }
  if (raw === '0' || lower === 'false' || lower === 'off') {
    return false
  }
  return undefined
}

export function formatSherpaLidTtsExclusionLabel(value: boolean | undefined): string {
  if (value === undefined) {
    return 'library default'
  }
  return value ? 'on' : 'off'
}

export function buildEchoLanguageIdConfig(
  lidModelPath: string,
  ttsExclusion?: boolean,
): EchoLanguageIdConfig {
  const base: EchoLanguageIdConfig = {
    modelPath: lidModelPath,
    minSpeechMs: CLOUD_DEFAULT_LID_MIN_SPEECH_MS,
  }
  if (ttsExclusion === undefined) {
    return base
  }
  return { ...base, ttsExclusion }
}

/** `SHERPA_MULTI_SESSIONS` (preferred) or legacy `SESSIONS`; default 10, clamped 1–20. */
export function resolveMultiSessionCount(env: NodeJS.ProcessEnv = process.env): number {
  const raw = env.SHERPA_MULTI_SESSIONS?.trim() || env.SESSIONS?.trim()
  const n = Number(raw ?? DEFAULT_MULTI_SESSIONS)
  if (!Number.isFinite(n)) {
    return DEFAULT_MULTI_SESSIONS
  }
  return Math.max(1, Math.min(MAX_MULTI_SESSIONS, Math.floor(n)))
}
