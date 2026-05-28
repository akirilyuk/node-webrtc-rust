/**
 * Live vendor presets for manual VoiceAgent testing.
 *
 * Each preset lists required env vars and default STT/TTS models.
 * Import from voice-agent live scripts; mirrored in SDK live tests.
 */

import type { SttConfig, TtsConfig, VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

export type LiveVendorId =
  | 'openai'
  | 'deepgram'
  | 'elevenlabs'
  | 'cartesia'
  | 'assemblyai'
  | 'google'

export interface LiveVendorPreset {
  id: LiveVendorId
  label: string
  /** Env vars that must be set (non-empty) before running. */
  requiredEnv: string[]
  /** Extra keys used when pairing STT/TTS across vendors. */
  optionalEnv?: string[]
  config: VoiceAgentConfig
  /** Sample phrase sent via sendTextToTTS in live demos. */
  ttsPhrase: string
  notes: string
}

function env(key: string): string | undefined {
  const value = process.env[key]
  return value && value.trim().length > 0 ? value.trim() : undefined
}

function withKeys(stt: SttConfig, tts: TtsConfig): VoiceAgentConfig {
  return {
    vad: { enabled: true, bargeIn: { enabled: true, flushTts: true } },
    events: { mode: 'both' },
    stt: { ...stt, apiKey: stt.apiKey ?? (stt.provider === 'openai' ? env('OPENAI_API_KEY') : undefined) },
    tts: { ...tts, apiKey: resolveTtsApiKey(tts) },
  }
}

function resolveTtsApiKey(tts: TtsConfig): string | undefined {
  if (tts.apiKey) return tts.apiKey
  switch (tts.provider) {
    case 'openai':
      return env('OPENAI_API_KEY')
    case 'elevenlabs':
      return env('ELEVENLABS_API_KEY')
    case 'cartesia':
      return env('CARTESIA_API_KEY')
    case 'google':
      return env('GOOGLE_API_KEY')
    default:
      return undefined
  }
}

export const LIVE_VENDOR_PRESETS: Record<LiveVendorId, LiveVendorPreset> = {
  openai: {
    id: 'openai',
    label: 'OpenAI',
    requiredEnv: ['OPENAI_API_KEY'],
    config: withKeys(
      { provider: 'openai', model: 'whisper-1', language: 'en' },
      { provider: 'openai', model: 'tts-1', voice: 'alloy' },
    ),
    ttsPhrase: 'OpenAI TTS live check. If you hear this, outbound injection works.',
    notes: 'STT + TTS both use OPENAI_API_KEY.',
  },
  deepgram: {
    id: 'deepgram',
    label: 'Deepgram STT',
    requiredEnv: ['DEEPGRAM_API_KEY', 'OPENAI_API_KEY'],
    config: withKeys(
      { provider: 'deepgram', model: 'nova-2', language: 'en', apiKey: env('DEEPGRAM_API_KEY') },
      { provider: 'openai', model: 'tts-1', voice: 'alloy' },
    ),
    ttsPhrase: 'Deepgram STT with OpenAI TTS pairing. Speak into the user leg to test transcription.',
    notes: 'Deepgram is STT-only in this SDK; TTS uses OpenAI for the demo pairing.',
  },
  elevenlabs: {
    id: 'elevenlabs',
    label: 'ElevenLabs TTS',
    requiredEnv: ['ELEVENLABS_API_KEY', 'OPENAI_API_KEY'],
    config: withKeys(
      { provider: 'openai', model: 'whisper-1', language: 'en' },
      {
        provider: 'elevenlabs',
        model: 'eleven_multilingual_v2',
        voice: process.env.ELEVENLABS_VOICE_ID ?? 'EXAVITQu4vr4xnSDxMaL',
        apiKey: env('ELEVENLABS_API_KEY'),
      },
    ),
    ttsPhrase: 'ElevenLabs text to speech live check.',
    notes: 'Set ELEVENLABS_VOICE_ID to override the default Rachel voice id.',
  },
  cartesia: {
    id: 'cartesia',
    label: 'Cartesia TTS',
    requiredEnv: ['CARTESIA_API_KEY', 'OPENAI_API_KEY'],
    config: withKeys(
      { provider: 'openai', model: 'whisper-1', language: 'en' },
      {
        provider: 'cartesia',
        model: 'sonic-english',
        voice: process.env.CARTESIA_VOICE_ID ?? 'default',
        apiKey: env('CARTESIA_API_KEY'),
      },
    ),
    ttsPhrase: 'Cartesia sonic live synthesis check.',
    notes: 'Set CARTESIA_VOICE_ID to your Cartesia voice id.',
  },
  assemblyai: {
    id: 'assemblyai',
    label: 'AssemblyAI STT',
    requiredEnv: ['ASSEMBLYAI_API_KEY', 'OPENAI_API_KEY'],
    config: withKeys(
      {
        provider: 'assemblyai',
        model: 'universal-streaming-english',
        language: 'en',
        apiKey: env('ASSEMBLYAI_API_KEY'),
      },
      { provider: 'openai', model: 'tts-1', voice: 'alloy' },
    ),
    ttsPhrase: 'AssemblyAI speech to text with OpenAI TTS pairing.',
    notes: 'AssemblyAI is STT-only; TTS uses OpenAI for the demo pairing.',
  },
  google: {
    id: 'google',
    label: 'Google Cloud Speech',
    requiredEnv: ['GOOGLE_APPLICATION_CREDENTIALS'],
    optionalEnv: ['GOOGLE_API_KEY'],
    config: withKeys(
      { provider: 'google', model: 'latest_long', language: 'en-US' },
      { provider: 'google', model: 'en-US-Neural2-A', voice: 'en-US-Neural2-A' },
    ),
    ttsPhrase: 'Google Cloud text to speech live check.',
    notes: 'Uses Application Default Credentials via GOOGLE_APPLICATION_CREDENTIALS.',
  },
}

export function getLiveVendorPreset(id: string): LiveVendorPreset | undefined {
  return LIVE_VENDOR_PRESETS[id as LiveVendorId]
}

export function listLiveVendorIds(): LiveVendorId[] {
  return Object.keys(LIVE_VENDOR_PRESETS) as LiveVendorId[]
}

export function missingEnvVars(preset: LiveVendorPreset): string[] {
  return preset.requiredEnv.filter((key) => !env(key))
}

export function liveVendorEnabled(id: LiveVendorId): boolean {
  if (process.env.VOICE_LIVE_TEST !== '1') return false
  if (process.env[`VOICE_LIVE_${id.toUpperCase()}`] !== '1') return false
  const preset = LIVE_VENDOR_PRESETS[id]
  return missingEnvVars(preset).length === 0
}
