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
 * Documented worst-case latency of semantic barge-in (`bargeIn.requireSttPartial`), in ms from
 * caller speech onset to `barge_in`, with the default local Sherpa streaming STT.
 *
 * Barge-in fires on the first partial with `minSttPartialTokens` (2) tokens. Partials arrive about
 * every 1280 ms; when the first one after onset holds one token, barge waits one more interval:
 * 1440 ms (time to 2 tokens) + 1280 ms = 2720 ms (about 2.6 s observed on CI).
 *
 * Lockstep with Rust `SEMANTIC_BARGE_IN_WORST_CASE_LATENCY_MS` (`crates/speech/src/config.rs`,
 * guarded by `crates/speech/tests/semantic_barge_latency_test.rs`).
 */
export const SEMANTIC_BARGE_IN_WORST_CASE_LATENCY_MS = 2720

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
 * Default `speakerGate.recheckMaxWindows`: windows scored per utterance while the verdict stays
 * `other`, the first check included. Mirrors `DEFAULT_RECHECK_MAX_WINDOWS` in the speech crate
 * (`crates/speech/src/speaker_gate.rs`); the Rust unit test cites this file back.
 */
export const SPEAKER_GATE_RECHECK_MAX_WINDOWS = 12

/**
 * Check windows of the 0.15.0 gate: the first check plus one re-check.
 * @deprecated the 0.15.0 window count; use `speakerCheckWindows` on the final.
 */
export const SPEAKER_GATE_RECHECK_WINDOWS = 2

/**
 * Earliest speech time (ms after the utterance's speech start) at which a barge-in released by
 * the `window`-th check window can come: `checkSpeechMs × window`. `window` is the
 * `speakerCheckWindows` of the final (1 = the first check). Mirrors `recheck_barge_earliest_ms`
 * in `crates/speech/src/speaker_gate.rs`.
 */
export function speakerRecheckBargeEarliestMs(checkSpeechMs: number, window: number): number {
  return checkSpeechMs * window
}
