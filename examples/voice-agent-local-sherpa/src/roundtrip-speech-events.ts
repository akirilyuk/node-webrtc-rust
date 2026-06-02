/**
 * Log every speech event emitted by Rust VoiceAgent during Sherpa roundtrip E2E.
 *
 * Mirrors the multi-client browser path (`speech_event` on the voice-control channel →
 * `appendEvent` in `client.js`). Logs all {@link SpeechEvent} types including
 * `vad_triggered`, `stt_stream_*`, and `user_stt_*` lifecycle events.
 * Enabled for all `start:roundtrip*` scripts unless
 * `SHERPA_ROUNDTRIP_EVENT_LOG=0` (including CI — rust `[voice-debug]` may be off).
 *
 * Opt out: `SHERPA_ROUNDTRIP_EVENT_LOG=0`
 */

import type { SpeechEvent } from '@node-webrtc-rust/sdk/voice'

let testStartedAtMs = Date.now()
let loggingEnabled = false

export function enableRoundtripSpeechEventLog(): void {
  loggingEnabled = true
  testStartedAtMs = Date.now()
}

export function isRoundtripSpeechEventLogEnabled(): boolean {
  if (process.env.SHERPA_ROUNDTRIP_EVENT_LOG === '0') {
    return false
  }
  return loggingEnabled || process.env.SHERPA_COUNTING_VERBOSE === '1'
}

/** One line per event on stderr (alongside `[voice-debug]` from Rust). */
export function logRoundtripSpeechEvent(agentLabel: string, event: SpeechEvent): void {
  if (!isRoundtripSpeechEventLogEnabled()) {
    return
  }
  const atMs = Date.now() - testStartedAtMs
  let suffix = ''
  if (event.text != null && event.text.length > 0) {
    suffix = ` ${JSON.stringify(event.text)}`
  } else if (event.error != null) {
    suffix =
      typeof event.error === 'string'
        ? ` ${JSON.stringify(event.error)}`
        : ` ${JSON.stringify(String(event.error))}`
  }
  console.error(`[speech] [${agentLabel}] +${atMs}ms ${event.type}${suffix}`)
}
