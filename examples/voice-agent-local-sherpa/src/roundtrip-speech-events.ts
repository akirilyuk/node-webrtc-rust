/**
 * Log speech events during Sherpa roundtrip E2E (browser `speech_event` parity).
 *
 * Focused on transcripts + playback — not vad/stt_stream lifecycle spam.
 * Enabled for all `start:roundtrip*` unless `SHERPA_ROUNDTRIP_EVENT_LOG=0`.
 */

import type { SpeechEvent } from '@node-webrtc-rust/sdk/voice'

import { logSpeechEvent, resetSpeechEventLogClock } from '../../shared/speech-event-log.js'

let loggingEnabled = false

export function enableRoundtripSpeechEventLog(): void {
  loggingEnabled = true
  resetSpeechEventLogClock()
}

export function isRoundtripSpeechEventLogEnabled(): boolean {
  if (process.env.SHERPA_ROUNDTRIP_EVENT_LOG === '0') {
    return false
  }
  return loggingEnabled || process.env.SHERPA_COUNTING_VERBOSE === '1'
}

/** One line per meaningful event on stderr. */
export function logRoundtripSpeechEvent(agentLabel: string, event: SpeechEvent): void {
  logSpeechEvent(agentLabel, event, isRoundtripSpeechEventLogEnabled)
}
