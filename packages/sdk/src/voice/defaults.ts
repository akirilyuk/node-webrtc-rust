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
  minSilenceDurationMs: 1300,
  speechPadMs: 300,
  gateStt: false,
  gateSttOpenOnPending: true,
  sttGateHoldMs: 1000,
  sttListenTimeoutMs: 4000,
  utteranceFinalizeTimeoutMs: 1500,
  bargeIn: {
    enabled: true,
    useVad: true,
    flushTts: true,
    requireSttPartial: true,
    minSttPartialChars: 2,
    /** 0 = no playback guard; rely on requireSttPartial for noise rejection. */
    agentPlaybackGuardMs: 0,
  },
}

/**
 * Recommended for typical voice agents (user STT + agent TTS + barge-in).
 *
 * Only change from {@link DEFAULT_VOICE_AGENT_VAD}: `gateStt: true` so STT is not fed during
 * silence and `user_speaking_end` follows gate hold + finalize (not the first brief pause).
 */
export const VOICE_AGENT_VAD_PRESET: VadConfig = {
  ...DEFAULT_VOICE_AGENT_VAD,
  gateStt: true,
}
