/**
 * Live vendor presets mirrored from examples/shared/voice-vendor-presets.ts
 * for SDK tests. Keep in sync when adding vendors.
 */

export type LiveVendorId =
  | 'openai'
  | 'deepgram'
  | 'elevenlabs'
  | 'cartesia'
  | 'assemblyai'
  | 'google'

export interface LiveVendorPresetMeta {
  id: LiveVendorId
  requiredEnv: string[]
  sttProvider: string
  ttsProvider: string
}

export const LIVE_VENDOR_METAS: LiveVendorPresetMeta[] = [
  {
    id: 'openai',
    requiredEnv: ['OPENAI_API_KEY'],
    sttProvider: 'openai',
    ttsProvider: 'openai',
  },
  {
    id: 'deepgram',
    requiredEnv: ['DEEPGRAM_API_KEY', 'OPENAI_API_KEY'],
    sttProvider: 'deepgram',
    ttsProvider: 'openai',
  },
  {
    id: 'elevenlabs',
    requiredEnv: ['ELEVENLABS_API_KEY', 'OPENAI_API_KEY'],
    sttProvider: 'openai',
    ttsProvider: 'elevenlabs',
  },
  {
    id: 'cartesia',
    requiredEnv: ['CARTESIA_API_KEY', 'OPENAI_API_KEY'],
    sttProvider: 'openai',
    ttsProvider: 'cartesia',
  },
  {
    id: 'assemblyai',
    requiredEnv: ['ASSEMBLYAI_API_KEY', 'OPENAI_API_KEY'],
    sttProvider: 'assemblyai',
    ttsProvider: 'openai',
  },
  {
    id: 'google',
    requiredEnv: ['GOOGLE_APPLICATION_CREDENTIALS'],
    sttProvider: 'google',
    ttsProvider: 'google',
  },
]

export function envPresent(key: string): boolean {
  const value = process.env[key]
  return Boolean(value && value.trim().length > 0)
}

export function liveVendorEnabled(id: LiveVendorId): boolean {
  if (process.env.VOICE_LIVE_TEST !== '1') return false
  if (process.env[`VOICE_LIVE_${id.toUpperCase()}`] !== '1') return false
  const meta = LIVE_VENDOR_METAS.find((m) => m.id === id)
  if (!meta) return false
  return meta.requiredEnv.every(envPresent)
}

export function voiceConfigForVendor(id: LiveVendorId) {
  switch (id) {
    case 'openai':
      return {
        stt: { provider: 'openai' as const, model: 'whisper-1', language: 'en' },
        tts: { provider: 'openai' as const, model: 'tts-1', voice: 'alloy' },
      }
    case 'deepgram':
      return {
        stt: { provider: 'deepgram' as const, model: 'nova-2', language: 'en' },
        tts: { provider: 'openai' as const, model: 'tts-1', voice: 'alloy' },
      }
    case 'elevenlabs':
      return {
        stt: { provider: 'openai' as const, model: 'whisper-1', language: 'en' },
        tts: { provider: 'elevenlabs' as const, model: 'eleven_multilingual_v2', voice: 'demo' },
      }
    case 'cartesia':
      return {
        stt: { provider: 'openai' as const, model: 'whisper-1', language: 'en' },
        tts: { provider: 'cartesia' as const, model: 'sonic-english', voice: 'default' },
      }
    case 'assemblyai':
      return {
        stt: {
          provider: 'assemblyai' as const,
          model: 'universal-streaming-english',
          language: 'en',
        },
        tts: { provider: 'openai' as const, model: 'tts-1', voice: 'alloy' },
      }
    case 'google':
      return {
        stt: { provider: 'google' as const, model: 'latest_long', language: 'en-US' },
        tts: { provider: 'google' as const, model: 'en-US-Neural2-A', voice: 'en-US-Neural2-A' },
      }
  }
}
