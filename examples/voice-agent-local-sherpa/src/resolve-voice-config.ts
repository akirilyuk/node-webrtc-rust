/**
 * Resolve VoiceAgent config for local Sherpa STT in the browser demo.
 *
 * Requires SHERPA_MODEL_PATH (run `npm run download-model` first).
 * Optional SHERPA_LANGUAGE sets stt.language when it matches the downloaded bundle.
 * TTS stays on mock — no cloud API keys needed.
 */

import { existsSync } from 'fs'
import { basename } from 'path'

import type { VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

import {
  applyVoiceDebugOverrides,
  logResolvedVoiceConfig,
} from '../../shared/voice-debug-config.js'

export interface ResolvedVoiceConfig {
  config: VoiceAgentConfig
  label: string
  modelPath: string
  language: string
}

function inferLanguageFromPath(modelPath: string): string {
  const dir = basename(modelPath).toLowerCase()
  if (dir.includes('-es-') || dir.includes('-es-kroko')) return 'es'
  if (dir.includes('-fr-') || dir.includes('-fr-kroko')) return 'fr'
  if (dir.includes('-de-') || dir.includes('-de-kroko')) return 'de'
  if (dir.includes('-zh-')) return 'zh'
  if (dir.includes('-ru-')) return 'ru'
  if (dir.includes('-bn-')) return 'bn'
  if (dir.includes('ar_en_id_ja')) return 'en'
  if (dir.includes('-en-')) return 'en'
  return 'en'
}

const LOCAL_SHERPA_VOICE_CONFIG = (
  modelPath: string,
  language: string,
): VoiceAgentConfig => ({
  stt: {
    provider: 'local-sherpa',
    language,
    modelPath,
  },
  tts: { provider: 'mock', voice: 'demo' },
  events: { mode: 'both' },
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
    console.error('1. Download weights (once), e.g.:')
    console.error(
      '   npm run download-model --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa',
    )
    console.error('   npm run download-model:es --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa')
    console.error('   npm run download-model --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa -- --list')
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

  const language =
    process.env.SHERPA_LANGUAGE?.trim() || inferLanguageFromPath(modelPath)

  const config = applyVoiceDebugOverrides(LOCAL_SHERPA_VOICE_CONFIG(modelPath, language))
  logResolvedVoiceConfig('local-sherpa', config, {
    SHERPA_MODEL_PATH: modelPath,
    SHERPA_LANGUAGE: language,
  })

  return {
    config,
    label: `local Sherpa-ONNX (${language}, browser mic → on-device STT)`,
    modelPath,
    language,
  }
}
