/**
 * Sherpa roundtrip — multi-language TTS → `user_language` spoken language ID.
 *
 * Server listener enables `languageId.modelPath` (Whisper tiny). Four sequential legs play
 * language-specific Sherpa TTS phrases; assert `user_language` ISO 639-1 per leg.
 *
 *   npm run start:roundtrip-language-id --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Env:
 *   SHERPA_LID_MODEL_PATH          Whisper tiny dir (npm run download-lid)
 *   SHERPA_LANGUAGE_ID_TIMEOUT_MS  per-leg wait (default 90000)
 */

import { existsSync } from 'fs'
import { dirname, join } from 'path'
import { fileURLToPath } from 'url'

import { VoiceAgent, SPEECH_EVENT_TYPE } from '@node-webrtc-rust/sdk/voice'
import type { SpeechEvent, VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import {
  AgentSpeakingEndLatch,
  installRoundtripWallClockTimeout,
  playSpeakerTtsWithPostSilence,
  postTtsSilenceSeconds,
  startSpeakerSpeechPump,
} from './roundtrip-counting.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import { logRoundtripSpeechEvent } from './roundtrip-speech-events.js'
import { streamSilence } from './pcm-relay.js'
import { resolveRoundtripVoiceConfig, withRoundtripHarnessSilence } from './resolve-voice-config.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
const EXAMPLE_ROOT = join(__dirname, '..')
const MODELS_DIR = join(EXAMPLE_ROOT, '.models')

const DEFAULT_TIMEOUT_MS = 90_000
const DEFAULT_WARMUP_S = 0.6

/** Piper bundles from sherpa-tts-model-catalog.json */
const TTS_BUNDLES: Record<string, string> = {
  en: 'vits-piper-en_US-amy-low',
  de: 'vits-piper-de_DE-thorsten-medium',
  es: 'vits-piper-es-glados-medium',
}

/** French Piper TTS is not in the catalog — English Piper + French phrase; Whisper LID classifies content. */
export const LANGUAGE_ID_LEGS = [
  { lang: 'en', phrase: 'Hello, how are you today?', ttsId: 'en' },
  { lang: 'de', phrase: 'Guten Tag, wie geht es Ihnen?', ttsId: 'de' },
  { lang: 'fr', phrase: 'Bonjour, comment allez-vous?', ttsId: 'en' },
  { lang: 'es', phrase: 'Hola, cómo estás hoy?', ttsId: 'es' },
] as const

function resolveTtsModelPath(ttsId: string): string {
  const bundle = TTS_BUNDLES[ttsId]
  if (!bundle) {
    throw new Error(`Unknown TTS id for language-id roundtrip: ${ttsId}`)
  }
  const modelPath = join(MODELS_DIR, bundle)
  if (!existsSync(modelPath)) {
    throw new Error(
      `TTS model missing for ${ttsId}: ${modelPath}\nRun: npm run download-tts --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa -- --lang=${ttsId}`,
    )
  }
  return modelPath
}

export function resolveLidModelPath(): string {
  const fromEnv = process.env.SHERPA_LID_MODEL_PATH?.trim()
  const defaultPath = join(MODELS_DIR, 'sherpa-onnx-whisper-tiny')
  const path = fromEnv || defaultPath
  if (!existsSync(path)) {
    throw new Error(
      `LID model missing: ${path}\nRun: npm run download-lid --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa`,
    )
  }
  return path
}

function speakerConfigForLeg(
  base: VoiceAgentConfig,
  leg: (typeof LANGUAGE_ID_LEGS)[number],
): VoiceAgentConfig {
  const ttsModelPath = resolveTtsModelPath(leg.ttsId)
  return withRoundtripHarnessSilence({
    ...base,
    stt: undefined,
    languageId: undefined,
    tts: {
      provider: 'local-sherpa',
      modelPath: ttsModelPath,
      voice: '0',
    },
    vad: { enabled: false },
    events: { mode: 'stream' },
  })
}

function listenerConfigWithLanguageId(
  base: VoiceAgentConfig,
  lidModelPath: string,
): VoiceAgentConfig {
  return withRoundtripHarnessSilence({
    ...base,
    languageId: {
      modelPath: lidModelPath,
    },
    events: { mode: 'stream' },
  })
}

async function collectEventsUntil(
  listener: VoiceAgent,
  predicate: (event: SpeechEvent) => boolean,
  timeoutMs: number,
): Promise<SpeechEvent[]> {
  const events: SpeechEvent[] = []
  const deadline = Date.now() + timeoutMs
  const stream = listener.speechEvents()
  const iterator = stream[Symbol.asyncIterator]()
  while (Date.now() < deadline) {
    const remaining = Math.max(1, deadline - Date.now())
    const next = await Promise.race([
      iterator.next(),
      new Promise<IteratorResult<SpeechEvent>>((resolve) =>
        setTimeout(() => resolve({ done: true, value: undefined }), remaining),
      ),
    ])
    if (next.done) break
    const event = next.value
    events.push(event)
    logRoundtripSpeechEvent('listener', event)
    if (predicate(event)) {
      return events
    }
  }
  return events
}

async function runLeg(
  leg: (typeof LANGUAGE_ID_LEGS)[number],
  baseConfig: VoiceAgentConfig,
  lidModelPath: string,
  timeoutMs: number,
): Promise<{ expected: string; detected: string }> {
  const { agentOut, userInbound, userOut, agentInbound, cleanup } =
    await createBidirectionalLoopback()

  const speaker = new VoiceAgent(speakerConfigForLeg(baseConfig, leg))
  const listener = new VoiceAgent(listenerConfigWithLanguageId(baseConfig, lidModelPath))
  const agentEndLatch = new AgentSpeakingEndLatch()

  await speaker.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await listener.attach({ inboundTrack: userInbound, outboundTrack: userOut })
  await speaker.start()
  await listener.start()

  startSpeakerSpeechPump(speaker, agentEndLatch)
  await streamSilence(agentOut, DEFAULT_WARMUP_S)

  const eventsPromise = collectEventsUntil(
    listener,
    (event) => event.type === SPEECH_EVENT_TYPE.userLanguage,
    timeoutMs,
  )

  await playSpeakerTtsWithPostSilence({
    speaker,
    speakerOut: agentOut,
    phrase: leg.phrase,
    postTtsSilenceS: postTtsSilenceSeconds(baseConfig),
    playbackTimeoutMs: timeoutMs,
    agentSpeakingEndLatch: agentEndLatch,
  })

  const events = await eventsPromise
  const langEvent = events.find((event) => event.type === SPEECH_EVENT_TYPE.userLanguage)
  const detected = langEvent?.language ?? langEvent?.text
  if (!detected) {
    throw new Error(`no user_language event for leg ${leg.lang}`)
  }

  await speaker.stop().catch(() => undefined)
  await listener.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  return { expected: leg.lang, detected }
}

export async function main(): Promise<void> {
  installRoundtripWallClockTimeout(120_000)
  const timeoutMs = Number(process.env.SHERPA_LANGUAGE_ID_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
  const lidModelPath = resolveLidModelPath()
  const { config: base, label, sttModelPath, ttsModelPath } = resolveRoundtripVoiceConfig()

  console.log('=== Sherpa spoken language ID roundtrip ===')
  console.log(`Pipeline: ${label}`)
  console.log(`STT: ${sttModelPath}`)
  console.log(`TTS (default): ${ttsModelPath}`)
  console.log(`LID: ${lidModelPath}`)
  console.log(`Legs: ${LANGUAGE_ID_LEGS.map((l) => l.lang).join(', ')}`)
  console.log(`Timeout per leg: ${timeoutMs} ms\n`)

  const results: Array<{ expected: string; detected: string; ok: boolean }> = []

  for (const leg of LANGUAGE_ID_LEGS) {
    console.log(`\n--- Leg ${leg.lang}: ${leg.phrase} ---`)
    try {
      const { expected, detected } = await runLeg(leg, base, lidModelPath, timeoutMs)
      const ok = detected.toLowerCase() === expected.toLowerCase()
      results.push({ expected, detected, ok })
      console.log(`user_language: ${detected} (expected ${expected}) ${ok ? 'OK' : 'FAIL'}`)
      if (!ok) {
        exitSherpaRoundtripFailure({
          reason: `user_language mismatch for ${expected}: got ${detected}`,
        })
      }
    } catch (error) {
      exitSherpaRoundtripFailure({
        reason: error instanceof Error ? error.message : String(error),
        error,
      })
    }
  }

  const failed = results.filter((r) => !r.ok)
  if (failed.length > 0) {
    exitSherpaRoundtripFailure({
      reason: `language-id roundtrip failed: ${failed.length} leg(s)`,
      failures: failed.map((r) => `${r.expected}: got ${r.detected}`),
    })
  }

  console.log('\n✓ All language-id legs passed')
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((error) => {
    exitSherpaRoundtripFailure({
      reason: error instanceof Error ? error.message : String(error),
      error,
    })
  })
}
