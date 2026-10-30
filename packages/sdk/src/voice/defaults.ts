/**
 * Default and recommended VAD presets for {@link VoiceAgent}.
 *
 * Values match Rust `VadConfig` / `BargeInConfig` defaults unless noted.
 *
 * @see [VOICE-VAD-AND-BARGE-IN.md](../../VOICE-VAD-AND-BARGE-IN.md) — tuning latency vs accuracy
 * @see [VOICE-API.md](../../VOICE-API.md) — API reference
 */

import type { VadConfig } from './types.js'

/**
 * Library defaults (`gateStt: false`).
 * Omit `vad` in {@link VoiceAgentConfig} to get the same behavior from Rust `VadConfig::default()`.
 */
export const DEFAULT_VOICE_AGENT_VAD: VadConfig = {
  enabled: true,
  provider: 'energy',
  threshold: 0.15,
  minSpeechDurationMs: 250,
  minSilenceDurationMs: 1100,
  speechPadMs: 300,
  gateStt: false,
  gateSttOpenOnPending: true,
  sttGateHoldMs: 400,
  sttListenTimeoutMs: 4000,
  utteranceFinalizeTimeoutMs: 1500,
  bargeIn: {
    enabled: true,
    useVad: true,
    flushTts: true,
    requireSttPartial: true,
    minSttPartialTokens: 2,
    /** 0 = no playback guard; rely on requireSttPartial for noise rejection. */
    agentPlaybackGuardMs: 0,
  },
}

/**
 * Recommended for typical voice agents (user STT + agent TTS + barge-in).
 *
 * Changes from {@link DEFAULT_VOICE_AGENT_VAD}:
 * - `gateStt: true` — STT only while the gate is open; `user_speaking_end` follows gate hold + finalize.
 * - `speechPadMs: 500` — larger pre-roll ring (with `minSpeechDurationMs`) for barge-in first syllable capture over agent TTS.
 */
export const VOICE_AGENT_VAD_PRESET: VadConfig = {
  ...DEFAULT_VOICE_AGENT_VAD,
  gateStt: true,
  speechPadMs: 500,
}

/**
 * Check windows a speaker-gate utterance can be scored on: the first check plus one re-check after
 * an `other` verdict (`speakerGate.recheckOnOther`). Mirrors `RECHECK_WINDOWS` in the speech crate
 * (`crates/speech/src/speaker_gate.rs`); both are asserted to be 2 in their own tests.
 */
export const SPEAKER_GATE_RECHECK_WINDOWS = 2

/**
 * Earliest speech time (ms after the utterance's speech start) at which a barge-in released by a
 * re-check can come: the end of the second check window, `2 × checkSpeechMs`. Mirrors
 * `recheck_barge_earliest_ms` in `crates/speech/src/speaker_gate.rs`.
 */
export function speakerRecheckBargeEarliestMs(checkSpeechMs: number): number {
  return checkSpeechMs * SPEAKER_GATE_RECHECK_WINDOWS
}
