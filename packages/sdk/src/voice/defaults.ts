/**
 * Default and recommended VAD presets for VoiceAgent.
 *
 * Values match Rust `VadConfig` / `BargeInConfig` defaults unless noted.
 * See packages/sdk/VOICE-VAD-AND-BARGE-IN.md for use cases and tuning.
 */

import type { VadConfig } from './types.js'

/** Library defaults (omit `vad` in VoiceAgentConfig to use the same in Rust). */
export const DEFAULT_VOICE_AGENT_VAD: VadConfig = {
  enabled: true,
  provider: 'energy',
  threshold: 0.15,
  minSpeechDurationMs: 250,
  minSilenceDurationMs: 500,
  speechPadMs: 300,
  gateStt: false,
  gateSttOpenOnPending: true,
  sttGateHoldMs: 1000,
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
 * Only change from library default: `gateStt: true`.
 */
export const VOICE_AGENT_VAD_PRESET: VadConfig = {
  ...DEFAULT_VOICE_AGENT_VAD,
  gateStt: true,
}
