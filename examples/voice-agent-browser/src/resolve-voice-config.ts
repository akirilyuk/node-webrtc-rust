/**
 * Resolve VoiceAgent config for the browser demo — mock by default, live when
 * `VOICE_VENDOR` is set (same presets as `examples/voice-agent`).
 */

import type { VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

import {
  getLiveVendorPreset,
  listLiveVendorIds,
  missingEnvVars,
  type LiveVendorId,
} from '../../shared/voice-vendor-presets.js'

const MOCK_VOICE_CONFIG: VoiceAgentConfig = {
  stt: { provider: 'mock', language: 'en' },
  tts: { provider: 'mock', voice: 'demo' },
  events: { mode: 'callback' },
  vad: {
    enabled: true,
    threshold: 0.05,
    minSpeechDurationMs: 80,
    bargeIn: { enabled: true, flushTts: true },
  },
}

export interface ResolvedVoiceConfig {
  config: VoiceAgentConfig
  label: string
  mode: 'mock' | 'live'
  vendorId?: LiveVendorId
}

/**
 * Returns mock config unless `VOICE_VENDOR` names a supported live preset.
 * Exits the process with a clear message when the vendor id or env vars are invalid.
 */
export function resolveVoiceConfig(): ResolvedVoiceConfig {
  const rawVendor = process.env.VOICE_VENDOR?.trim()
  if (!rawVendor) {
    return { config: MOCK_VOICE_CONFIG, label: 'mock (no API keys)', mode: 'mock' }
  }

  const preset = getLiveVendorPreset(rawVendor)
  if (!preset) {
    console.error(
      `Unknown VOICE_VENDOR="${rawVendor}". Supported: ${listLiveVendorIds().join(', ')}`,
    )
    process.exit(1)
  }

  const missing = missingEnvVars(preset)
  if (missing.length > 0) {
    console.error(`Missing required env for ${preset.label}: ${missing.join(', ')}`)
    console.error(preset.notes)
    console.error('See examples/voice-agent-browser/README.md for per-vendor commands.')
    process.exit(1)
  }

  return {
    config: preset.config,
    label: preset.label,
    mode: 'live',
    vendorId: preset.id,
  }
}
