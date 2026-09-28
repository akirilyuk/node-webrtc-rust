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
  | 'groq'
  | 'azure'
  | 'aws'

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
    requiredEnv: ['DEEPGRAM_API_KEY'],
    sttProvider: 'deepgram',
    ttsProvider: 'deepgram',
  },
  {
    id: 'elevenlabs',
    requiredEnv: ['ELEVENLABS_API_KEY'],
    sttProvider: 'elevenlabs',
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
  {
    id: 'groq',
    requiredEnv: ['GROQ_API_KEY'],
    sttProvider: 'groq',
    ttsProvider: 'groq',
  },
  {
    id: 'azure',
    requiredEnv: ['AZURE_SPEECH_KEY', 'AZURE_SPEECH_REGION', 'AZURE_SPEECH_RESOURCE'],
    sttProvider: 'azure',
    ttsProvider: 'azure',
  },
  {
    id: 'aws',
    requiredEnv: ['AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY', 'AWS_REGION'],
    sttProvider: 'aws',
    ttsProvider: 'aws',
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
        tts: { provider: 'deepgram' as const, model: 'aura-asteria-en', voice: 'aura-asteria-en' },
      }
    case 'elevenlabs':
      return {
        stt: { provider: 'elevenlabs' as const, model: 'scribe_v2_realtime', language: 'en' },
        tts: { provider: 'elevenlabs' as const, model: 'eleven_multilingual_v2', voice: 'demo' },
      }
    case 'cartesia':
      return {
        stt: { provider: 'openai' as const, model: 'whisper-1', language: 'en' },
        tts: { provider: 'cartesia' as const, model: 'sonic-3', voice: 'default' },
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
    case 'groq':
      return {
        stt: { provider: 'groq' as const, model: 'whisper-large-v3-turbo', language: 'en' },
        tts: {
          provider: 'groq' as const,
          model: 'canopylabs/orpheus-v1-english',
          voice: 'troy',
        },
      }
    case 'azure':
      return {
        stt: { provider: 'azure' as const, model: 'conversation', language: 'en-US' },
        tts: { provider: 'azure' as const, model: 'en-US-JennyNeural', voice: 'en-US-JennyNeural' },
      }
    case 'aws':
      return {
        stt: { provider: 'aws' as const, model: 'en-US', language: 'en-US' },
        tts: { provider: 'aws' as const, model: 'Joanna', voice: 'Joanna' },
      }
  }
}
