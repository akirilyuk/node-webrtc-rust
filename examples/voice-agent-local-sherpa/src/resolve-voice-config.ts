/**
 * Resolve VoiceAgent config for local Sherpa STT + TTS in the browser demo.
 *
 * VAD/barge-in values follow packages/sdk/VOICE-VAD-AND-BARGE-IN.md (preset + energy
 * threshold 0.05). Override via env only when debugging — defaults are meant to work
 * without tuning.
 *
 * Requires SHERPA_STT_MODEL_PATH (run `npm run download-stt` first).
 * Requires SHERPA_TTS_MODEL_PATH (run `npm run download-tts` first).
 * Optional SHERPA_STT_LANGUAGE sets stt.language when it matches the downloaded STT bundle.
 * Optional SHERPA_TTS_SPEAKER sets Piper speaker id (default 0).
 *
 * Deprecated aliases (still read): SHERPA_MODEL_PATH, SHERPA_LANGUAGE.
 */

import { existsSync } from 'fs'
import { basename } from 'path'

import type { VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'
import { VOICE_AGENT_VAD_PRESET } from '@node-webrtc-rust/sdk/voice'

import {
  applyVoiceDebugOverrides,
  logResolvedVoiceConfig,
} from '../../shared/voice-debug-config.js'

export interface ResolvedVoiceConfig {
  config: VoiceAgentConfig
  label: string
  sttModelPath: string
  ttsModelPath: string
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
  sttModelPath: string,
  ttsModelPath: string,
  language: string,
  speaker: string,
): VoiceAgentConfig => ({
  stt: {
    provider: 'local-sherpa',
    language,
    modelPath: sttModelPath,
  },
  tts: {
    provider: 'local-sherpa',
    modelPath: ttsModelPath,
    voice: speaker,
  },
  events: { mode: 'both' },
  vad: {
    ...VOICE_AGENT_VAD_PRESET,
    provider: 'energy',
    // Energy RMS scale on loopback — lower than default 0.15 for reliable Sherpa demos.
    threshold: 0.05,
  },
})

function requireDirectory(path: string | undefined, envName: string, downloadHint: string): string {
  const trimmed = path?.trim()
  if (!trimmed) {
    console.error(`${envName} is not set.\n`)
    console.error(downloadHint)
    process.exit(1)
  }
  if (!existsSync(trimmed)) {
    console.error(`${envName} does not exist: ${trimmed}`)
    console.error(downloadHint)
    process.exit(1)
  }
  return trimmed
}

/**
 * Loads local Sherpa config from SHERPA_STT_MODEL_PATH and SHERPA_TTS_MODEL_PATH.
 */
export function resolveVoiceConfig(): ResolvedVoiceConfig {
  const sttDownloadHint =
    'Run: npm run download-stt --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa'
  const ttsDownloadHint =
    'Run: npm run download-tts --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa'

  const sttModelPath = requireDirectory(
    process.env.SHERPA_STT_MODEL_PATH ?? process.env.SHERPA_MODEL_PATH,
    'SHERPA_STT_MODEL_PATH',
    sttDownloadHint,
  )
  const ttsModelPath = requireDirectory(
    process.env.SHERPA_TTS_MODEL_PATH,
    'SHERPA_TTS_MODEL_PATH',
    ttsDownloadHint,
  )

  const language =
    process.env.SHERPA_STT_LANGUAGE?.trim() ??
    process.env.SHERPA_LANGUAGE?.trim() ??
    inferLanguageFromPath(sttModelPath)
  const speaker = process.env.SHERPA_TTS_SPEAKER?.trim() || '0'

  const config = applyVoiceDebugOverrides(
    LOCAL_SHERPA_VOICE_CONFIG(sttModelPath, ttsModelPath, language, speaker),
  )
  logResolvedVoiceConfig('local-sherpa', config, {
    SHERPA_STT_MODEL_PATH: sttModelPath,
    SHERPA_TTS_MODEL_PATH: ttsModelPath,
    SHERPA_STT_LANGUAGE: language,
    SHERPA_TTS_SPEAKER: speaker,
  })

  return {
    config,
    label: `local Sherpa-ONNX (${language}, on-device STT + TTS)`,
    sttModelPath,
    ttsModelPath,
    language,
  }
}
