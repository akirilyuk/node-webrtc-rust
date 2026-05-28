/**
 * Shared VoiceAgent debug overrides for browser voice examples.
 *
 * Enable with `VOICE_DEBUG=1` (see `start:debug` npm scripts).
 */

import {
  isVoiceDebugEnabled,
  voiceDebugLog,
  type VoiceAgentConfig,
} from '@node-webrtc-rust/sdk/voice'

/** Applies looser VAD and explicit `gateStt: false` when debug mode is on. */
export function applyVoiceDebugOverrides(config: VoiceAgentConfig): VoiceAgentConfig {
  if (!isVoiceDebugEnabled()) {
    return config
  }

  const vadDisabled = process.env.VOICE_VAD_DISABLED === '1'
  const thresholdRaw = process.env.VOICE_VAD_THRESHOLD?.trim()
  const parsedThreshold = thresholdRaw ? Number.parseFloat(thresholdRaw) : 0.01
  const threshold = Number.isFinite(parsedThreshold) ? parsedThreshold : 0.01

  return {
    ...config,
    vad: {
      ...config.vad,
      enabled: vadDisabled ? false : (config.vad?.enabled ?? true),
      threshold,
      minSpeechDurationMs: 40,
      minSilenceDurationMs: 400,
      gateStt: false,
    },
  }
}

/** Prints resolved VoiceAgent config (secrets redacted) at server startup. */
export function logResolvedVoiceConfig(
  label: string,
  config: VoiceAgentConfig,
  extras?: Record<string, string>,
): void {
  if (!isVoiceDebugEnabled()) {
    return
  }

  voiceDebugLog('config', label)
  if (extras) {
    for (const [key, value] of Object.entries(extras)) {
      voiceDebugLog('config', `${key}=${value}`)
    }
  }

  const stt = config.stt
    ? {
        ...config.stt,
        apiKey: config.stt.apiKey ? '[redacted]' : undefined,
      }
    : undefined

  voiceDebugLog('config', JSON.stringify({ ...config, stt }, null, 2))
}
