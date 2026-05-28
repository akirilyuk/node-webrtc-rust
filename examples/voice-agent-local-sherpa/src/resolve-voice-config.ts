/**
 * Resolve VoiceAgent config for local Sherpa STT in the browser demo.
 *
 * Requires SHERPA_MODEL_PATH (run `npm run download-model` first).
 * TTS stays on mock — no cloud API keys needed.
 */

import { existsSync } from 'fs'

import type { VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

export interface ResolvedVoiceConfig {
  config: VoiceAgentConfig
  label: string
  modelPath: string
}

const LOCAL_SHERPA_VOICE_CONFIG = (modelPath: string): VoiceAgentConfig => ({
  stt: {
    provider: 'local-sherpa',
    language: 'en',
    modelPath,
  },
  tts: { provider: 'mock', voice: 'demo' },
  events: { mode: 'callback' },
  vad: {
    enabled: true,
    threshold: 0.05,
    minSpeechDurationMs: 80,
    bargeIn: { enabled: true, flushTts: true },
  },
})

/**
 * Loads local Sherpa config from SHERPA_MODEL_PATH.
 * Exits with setup instructions when the path is missing or invalid.
 */
export function resolveVoiceConfig(): ResolvedVoiceConfig {
  const modelPath = process.env.SHERPA_MODEL_PATH?.trim()
  if (!modelPath) {
    console.error('SHERPA_MODEL_PATH is not set.\n')
    console.error('1. Download weights (once):')
    console.error(
      '   npm run download-model --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa',
    )
    console.error('2. Export the path printed by the script, e.g.:')
    console.error(
      '   export SHERPA_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-en-2023-06-26"',
    )
    console.error('3. Start the server:')
    console.error(
      '   npm run start --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa',
    )
    console.error('\nSee examples/voice-agent-local-sherpa/README.md for full walkthrough.')
    process.exit(1)
  }

  if (!existsSync(modelPath)) {
    console.error(`SHERPA_MODEL_PATH does not exist: ${modelPath}`)
    console.error('Run download-model or point to a directory containing tokens.txt and *.onnx files.')
    process.exit(1)
  }

  return {
    config: LOCAL_SHERPA_VOICE_CONFIG(modelPath),
    label: 'local Sherpa-ONNX (browser mic → on-device STT)',
    modelPath,
  }
}
