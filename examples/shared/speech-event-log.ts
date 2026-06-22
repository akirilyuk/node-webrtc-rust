/**
 * Focused VoiceAgent speech logging: recognized text, TTS speak lines, playback markers.
 *
 * Omits low-level lifecycle (`vad_triggered`, `stt_stream_*`, `user_stt_*`, …).
 * `user_speech_partial` is logged only when the transcript text changes (not every Sherpa decode).
 *
 * Opt out: `SPEECH_EVENT_LOG=0`
 */
import type { SpeechEvent } from '@node-webrtc-rust/sdk/voice'

let testStartedAtMs = Date.now()
const lastPartialByLabel = new Map<string, string>()

/** Low-level pipeline events — skip in default speech log. */
const LIFECYCLE_ONLY = new Set<SpeechEvent['type']>([
  'user_speaking_start',
  'user_speaking_end',
  'vad_triggered',
  'stt_stream_start',
  'stt_stream_end',
  'user_stt_start',
  'user_stt_end',
  'user_stt_not_found',
])

export function resetSpeechEventLogClock(): void {
  testStartedAtMs = Date.now()
  lastPartialByLabel.clear()
}

export function isSpeechEventLogEnabled(): boolean {
  if (process.env.SPEECH_EVENT_LOG === '0') return false
  return process.env.SPEECH_EVENT_LOG === '1'
}

function isEnabled(enabled?: () => boolean): boolean {
  return enabled?.() ?? isSpeechEventLogEnabled()
}

function formatAtMs(): number {
  return Date.now() - testStartedAtMs
}

function writeSpeechLine(agentLabel: string, atMs: number, kind: string, text?: string): void {
  const suffix = text != null && text.length > 0 ? ` ${JSON.stringify(text)}` : ''
  console.error(`[speech] [${agentLabel}] +${atMs}ms ${kind}${suffix}`)
}

/** Log text queued for TTS (`ctx.speak` / `sendTextToTTS`). */
export function logSpeakText(
  agentLabel: string,
  text: string,
  enabled?: () => boolean,
): void {
  if (!isEnabled(enabled)) return
  writeSpeechLine(agentLabel, formatAtMs(), 'speak', text)
}

/** Log a VoiceAgent speech event (recognized text, playback, barge-in, errors). */
export function logSpeechEvent(
  agentLabel: string,
  event: SpeechEvent,
  enabled?: () => boolean,
): void {
  if (!isEnabled(enabled)) return
  if (LIFECYCLE_ONLY.has(event.type)) return

  if (event.type === 'user_speech_partial') {
    const trimmed = event.text?.trim() ?? ''
    if (!trimmed) return
    if (lastPartialByLabel.get(agentLabel) === trimmed) return
    lastPartialByLabel.set(agentLabel, trimmed)
  }

  if (event.type === 'user_speech_final') {
    lastPartialByLabel.delete(agentLabel)
  }

  const atMs = formatAtMs()
  if (event.text != null && event.text.length > 0) {
    writeSpeechLine(agentLabel, atMs, event.type, event.text)
    return
  }
  if (event.error != null) {
    const err =
      typeof event.error === 'string' ? event.error : String(event.error)
    writeSpeechLine(agentLabel, atMs, event.type, err)
    return
  }
  writeSpeechLine(agentLabel, atMs, event.type)
}
