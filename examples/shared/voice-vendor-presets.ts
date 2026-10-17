/**
 * Live vendor presets for manual VoiceAgent testing.
 *
 * ## Why this file exists
 *
 * Each cloud provider needs different env vars, models, and often **STT/TTS pairing**
 * because not every vendor supports both directions in this SDK:
 *
 * | Provider   | STT in SDK | TTS in SDK | Typical pairing in demos      |
 * |------------|------------|------------|-------------------------------|
 * | openai     | yes        | yes        | OpenAI + OpenAI               |
 * | deepgram   | yes        | yes        | Deepgram listen + Aura TTS    |
 * | elevenlabs | yes        | yes        | Scribe STT + ElevenLabs TTS   |
 * | cartesia   | no         | yes        | OpenAI STT + Cartesia TTS     |
 * | fishaudio  | no         | yes        | OpenAI STT + Fish Audio TTS   |
 * | fishaudio-stt | yes     | yes        | Fish STT + Fish Audio TTS     |
 * | assemblyai | yes        | no         | AssemblyAI STT + OpenAI TTS   |
 * | google     | yes        | yes        | Google STT + Google TTS       |
 * | groq       | yes        | yes        | Groq Whisper file + Orpheus   |
 * | azure      | yes        | yes        | Azure REST STT + REST TTS     |
 * | aws        | yes        | yes        | Transcribe stream + Polly     |
 *
 * Production apps can mix providers freely via `VoiceAgentConfig.stt` / `.tts`.
 *
 * ## API keys
 *
 * Keys are read from env at runtime (or explicit `apiKey` in config). They are never
 * logged. Prefer env vars in local dev; inject secrets from your worker orchestrator
 * in production.
 *
 * ## Mirrored in tests
 *
 * `packages/sdk/tests/voice-vendor-presets.ts` duplicates the vendor list for vitest.
 * Keep both in sync when adding a provider.
 *
 * Used by: `examples/voice-agent/src/live-vendor.ts` and npm `start:live:*` scripts.
 *
 * Official API docs for every provider: `examples/shared/VOICE_VENDOR_REFERENCE.md`
 */

import {
  VOICE_AGENT_VAD_PRESET,
  type SttConfig,
  type TtsConfig,
  type VadConfig,
  type VoiceAgentConfig,
} from '@node-webrtc-rust/sdk/voice'

export type LiveVendorId =
  | 'openai'
  | 'deepgram'
  | 'elevenlabs'
  | 'cartesia'
  | 'fishaudio'
  | 'fishaudio-stt'
  | 'assemblyai'
  | 'google'
  | 'groq'
  | 'azure'
  | 'aws'

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

/**
 * Shared defaults for live demos: VAD + barge-in on, deliver events both ways. `vad` replaces
 * the default VAD block (the Fish STT preset passes `VOICE_AGENT_VAD_PRESET` for `gateStt`).
 */
function withKeys(stt: SttConfig, tts: TtsConfig, vad?: VadConfig): VoiceAgentConfig {
  return {
    vad: vad ?? { enabled: true, bargeIn: { enabled: true, flushTts: true } },
    events: { mode: 'both' },
    stt: {
      ...stt,
      // OpenAI STT reads OPENAI_API_KEY when apiKey omitted; other STT vendors set explicitly below.
      apiKey: stt.apiKey ?? (stt.provider === 'openai' ? env('OPENAI_API_KEY') : undefined),
    },
    tts: { ...tts, apiKey: resolveTtsApiKey(tts) },
  }
}

function resolveTtsApiKey(tts: TtsConfig): string | undefined {
  if (tts.apiKey) return tts.apiKey
  switch (tts.provider) {
    case 'openai':
      return env('OPENAI_API_KEY')
    case 'deepgram':
      return env('DEEPGRAM_API_KEY')
    case 'elevenlabs':
      return env('ELEVENLABS_API_KEY')
    case 'cartesia':
      return env('CARTESIA_API_KEY')
    case 'fishaudio':
      return env('FISH_API_KEY')
    case 'google':
      return env('GOOGLE_API_KEY')
    case 'groq':
      return env('GROQ_API_KEY')
    case 'azure':
      return env('AZURE_SPEECH_KEY') ?? env('SPEECH_KEY')
    case 'aws':
      return undefined
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
    label: 'Deepgram (listen + Aura TTS)',
    requiredEnv: ['DEEPGRAM_API_KEY'],
    config: withKeys(
      { provider: 'deepgram', model: 'nova-2', language: 'en', apiKey: env('DEEPGRAM_API_KEY') },
      {
        provider: 'deepgram',
        model: 'aura-asteria-en',
        voice: 'aura-asteria-en',
        apiKey: env('DEEPGRAM_API_KEY'),
      },
    ),
    ttsPhrase:
      'Deepgram Nova listen with Aura speech. Speak into the user leg to test partials and finals.',
    notes:
      'Single DEEPGRAM_API_KEY. Listen models: nova-2 / nova-3 only (Flux listen not documented). TTS: aura-* on /v1 or flux-* on /v2.',
  },
  elevenlabs: {
    id: 'elevenlabs',
    label: 'ElevenLabs (Scribe + TTS)',
    requiredEnv: ['ELEVENLABS_API_KEY'],
    config: withKeys(
      {
        provider: 'elevenlabs',
        model: 'scribe_v2_realtime',
        language: 'en',
        apiKey: env('ELEVENLABS_API_KEY'),
      },
      {
        provider: 'elevenlabs',
        model: 'eleven_multilingual_v2',
        voice: process.env.ELEVENLABS_VOICE_ID ?? 'EXAVITQu4vr4xnSDxMaL',
        apiKey: env('ELEVENLABS_API_KEY'),
      },
    ),
    ttsPhrase: 'ElevenLabs Scribe and text to speech live check.',
    notes:
      'Single ELEVENLABS_API_KEY. Set ELEVENLABS_VOICE_ID to override the default Rachel voice id.',
  },
  cartesia: {
    id: 'cartesia',
    label: 'Cartesia TTS',
    requiredEnv: ['CARTESIA_API_KEY', 'OPENAI_API_KEY'],
    config: withKeys(
      { provider: 'openai', model: 'whisper-1', language: 'en' },
      {
        provider: 'cartesia',
        model: 'sonic-3',
        voice: process.env.CARTESIA_VOICE_ID ?? 'default',
        apiKey: env('CARTESIA_API_KEY'),
      },
    ),
    ttsPhrase: 'Cartesia sonic live synthesis check.',
    notes: 'Set CARTESIA_VOICE_ID to your Cartesia voice id. STT uses OpenAI for the demo pairing.',
  },
  fishaudio: {
    id: 'fishaudio',
    label: 'Fish Audio TTS (Drama 3)',
    requiredEnv: ['FISH_API_KEY', 'OPENAI_API_KEY'],
    config: withKeys(
      { provider: 'openai', model: 'whisper-1', language: 'en' },
      {
        provider: 'fishaudio',
        model: 'drama-3',
        voice: process.env.FISH_VOICE_ID ?? 'default',
        apiKey: env('FISH_API_KEY'),
      },
    ),
    ttsPhrase: 'Fish Audio Drama three live synthesis check.',
    notes:
      'FISH_VOICE_ID optional (Fish default voice when unset). STT uses OpenAI for the demo pairing.',
  },
  'fishaudio-stt': {
    id: 'fishaudio-stt',
    label: 'Fish Audio STT (transcribe-1) + TTS (Drama 3)',
    requiredEnv: ['FISH_API_KEY'],
    config: withKeys(
      { provider: 'fishaudio', model: 'transcribe-1', apiKey: env('FISH_API_KEY') },
      {
        provider: 'fishaudio',
        model: 'drama-3',
        voice: process.env.FISH_VOICE_ID ?? 'default',
        apiKey: env('FISH_API_KEY'),
      },
      // gateStt: true streams only VAD-gated speech to Fish, which bills STT for the audio sent.
      // Fish sends no partials, so barge-in must not wait for one (VAD alone interrupts TTS).
      {
        ...VOICE_AGENT_VAD_PRESET,
        bargeIn: { ...VOICE_AGENT_VAD_PRESET.bargeIn, requireSttPartial: false },
      },
    ),
    ttsPhrase: 'Fish Audio transcribe and Drama three live check.',
    notes:
      'Single FISH_API_KEY. Fish STT returns finals only (manual commit after VAD end). Fish STT sends no partial transcripts, so this preset sets bargeIn.requireSttPartial to false; without that, barge-in never fires during agent speech. No language option: Fish auto-detects.',
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
  groq: {
    id: 'groq',
    label: 'Groq',
    requiredEnv: ['GROQ_API_KEY'],
    config: withKeys(
      {
        provider: 'groq',
        model: 'whisper-large-v3-turbo',
        language: 'en',
        apiKey: env('GROQ_API_KEY'),
      },
      {
        provider: 'groq',
        model: 'canopylabs/orpheus-v1-english',
        voice: 'troy',
        apiKey: env('GROQ_API_KEY'),
      },
    ),
    ttsPhrase: 'Groq Orpheus text to speech live check.',
    notes:
      'File STT on VAD finalize only — no live partials. TTS is full-body WAV then framed for playback.',
  },
  azure: {
    id: 'azure',
    label: 'Azure AI Speech',
    requiredEnv: ['AZURE_SPEECH_KEY', 'AZURE_SPEECH_REGION', 'AZURE_SPEECH_RESOURCE'],
    optionalEnv: ['SPEECH_KEY'],
    config: withKeys(
      {
        provider: 'azure',
        model: 'conversation',
        language: process.env.AZURE_SPEECH_LANGUAGE ?? 'en-US',
        apiKey: env('AZURE_SPEECH_KEY') ?? env('SPEECH_KEY'),
        endpoint: env('AZURE_SPEECH_RESOURCE'),
      },
      {
        provider: 'azure',
        model: 'en-US-JennyNeural',
        voice: 'en-US-JennyNeural',
        apiKey: env('AZURE_SPEECH_KEY') ?? env('SPEECH_KEY'),
        endpoint: process.env.AZURE_SPEECH_REGION
          ? `${process.env.AZURE_SPEECH_REGION}.tts.speech.microsoft.com`
          : undefined,
      },
    ),
    ttsPhrase: 'Azure neural text to speech live check.',
    notes:
      'REST short-audio STT — finals only, no user_speech_partial. Same key for STT resource host and regional TTS.',
  },
  aws: {
    id: 'aws',
    label: 'AWS Transcribe + Polly',
    requiredEnv: ['AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY', 'AWS_REGION'],
    optionalEnv: ['AWS_SESSION_TOKEN'],
    config: withKeys(
      { provider: 'aws', model: 'en-US', language: 'en-US' },
      { provider: 'aws', model: 'Joanna', voice: 'Joanna' },
    ),
    ttsPhrase: 'Amazon Polly neural text to speech live check.',
    notes:
      'Standard AWS credential chain. Transcribe streaming partials; Polly SynthesizeSpeech full-body PCM.',
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

/** Same gate as SDK `voice-live.test.ts` — global flag + per-vendor flag + credentials. */
export function liveVendorEnabled(id: LiveVendorId): boolean {
  if (process.env.VOICE_LIVE_TEST !== '1') return false
  if (process.env[`VOICE_LIVE_${id.toUpperCase()}`] !== '1') return false
  const preset = LIVE_VENDOR_PRESETS[id]
  return missingEnvVars(preset).length === 0
}
