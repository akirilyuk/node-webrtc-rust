/**
 * Sherpa roundtrip with session audio recording for human listen-back.
 *
 * Captures stereo audio (L=speaker outbound TTS, R=listener inbound) via
 * {@link SessionRecorder} from `@node-webrtc-rust/helpers`.
 *
 * Run (WAV — bit-exact listen-back):
 *   NWR_RECORD_FORMAT=wav npm run start:record --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Run (Opus in Ogg @ NWR_RECORD_BITRATE_BPS, default 256000):
 *   NWR_RECORD_FORMAT=opus npm run start:record --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Output:
 *   .recordings/sherpa-roundtrip-record.wav | .ogg
 *   Printed as LISTEN_BACK_WAV=… or LISTEN_BACK_OPUS=… on success.
 */

import { mkdirSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import {
  resolveSessionRecorderOptionsFromEnv,
  SessionRecorder,
  type SessionRecorderFormat,
} from '@node-webrtc-rust/helpers'
import type { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import { VoiceAgent, VOICE_AGENT_VAD_PRESET } from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { streamSilence } from './pcm-relay.js'
import { resolveRoundtripVoiceConfig } from './resolve-voice-config.js'
import {
  AgentSpeakingEndLatch,
  DEFAULT_COUNTING_PHRASE_ONE_TO_TEN,
  DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
  evaluateCountingRoundtrip,
  installRoundtripWallClockTimeout,
  ListenerUtteranceCollector,
  NUMBER_WORDS_ONE_TO_TEN,
  playSpeakerTtsWithPostSilence,
  postTtsSilenceSeconds,
  roundtripWallClockMs,
  startSpeakerSpeechPump,
  sttFinalizeWaitMs,
} from './roundtrip-counting.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
const RECORDINGS_DIR = resolve(__dirname, '../.recordings')

export const DEFAULT_RECORD_WAV_PATH = resolve(RECORDINGS_DIR, 'sherpa-roundtrip-record.wav')
export const DEFAULT_RECORD_OPUS_PATH = resolve(RECORDINGS_DIR, 'sherpa-roundtrip-record.ogg')

function defaultRecordPath(format: SessionRecorderFormat): string {
  return format === 'opus' ? DEFAULT_RECORD_OPUS_PATH : DEFAULT_RECORD_WAV_PATH
}

const DEFAULT_TIMEOUT_MS = 45_000
const DEFAULT_MIN_NUMBER_WORDS = 6
const DEFAULT_WARMUP_S = 0.6

/** Soft L/R channel checks on finalized WAV (does not block write). */
export function evaluateRecordedWavChannels(params: {
  wav: Buffer
  outboundFrames: number
  inboundFrames: number
}): { passed: boolean; warnings: string[] } {
  const warnings: string[] = []
  if (params.outboundFrames === 0) {
    warnings.push('no outbound frames captured (L channel may be silent)')
  }
  if (params.inboundFrames === 0) {
    warnings.push('no inbound frames captured (R channel may be silent)')
  }
  if (params.wav.byteLength <= 44) {
    warnings.push('WAV payload empty')
    return { passed: false, warnings }
  }
  const view = new DataView(
    params.wav.buffer,
    params.wav.byteOffset + 44,
    params.wav.byteLength - 44,
  )
  const frames = Math.floor(view.byteLength / 4)
  let maxL = 0
  let maxR = 0
  for (let i = 0; i < frames; i++) {
    maxL = Math.max(maxL, Math.abs(view.getInt16(i * 4, true)))
    maxR = Math.max(maxR, Math.abs(view.getInt16(i * 4 + 2, true)))
  }
  if (params.outboundFrames > 0 && maxL < 200) {
    warnings.push(`L peak ${maxL} low despite outbound frames=${params.outboundFrames}`)
  }
  if (params.inboundFrames > 0 && maxR < 200) {
    warnings.push(`R peak ${maxR} low despite inbound frames=${params.inboundFrames}`)
  }
  return { passed: warnings.length === 0, warnings }
}

async function main(): Promise<void> {
  const { config, label, sttModelPath, ttsModelPath } = resolveRoundtripVoiceConfig()
  installRoundtripWallClockTimeout(roundtripWallClockMs(config, 'short'))

  const phrase = process.env.SHERPA_RECORD_PHRASE?.trim() || DEFAULT_COUNTING_PHRASE_ONE_TO_TEN
  const timeoutMs = Number(process.env.SHERPA_COUNTING_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
  const minNumberWords = Number(
    process.env.SHERPA_COUNTING_MIN_NUMBER_WORDS ?? DEFAULT_MIN_NUMBER_WORDS,
  )
  const finalizeWaitMs = sttFinalizeWaitMs(config)
  const postTtsSilenceS = postTtsSilenceSeconds(config)
  const verbose = process.env.SHERPA_COUNTING_VERBOSE === '1'

  const recorderOptions = resolveSessionRecorderOptionsFromEnv()
  const format: SessionRecorderFormat = recorderOptions.format ?? 'opus'
  recorderOptions.format = format
  const outputPath =
    process.env.NWR_RECORD_OUTPUT_PATH?.trim() || defaultRecordPath(format)

  console.log('=== Sherpa roundtrip + session record ===')
  console.log(`Pipeline: ${label}`)
  console.log(
    `Listener: gateStt=${config.vad?.gateStt !== false}  sttGateHold=${config.vad?.sttGateHoldMs ?? VOICE_AGENT_VAD_PRESET.sttGateHoldMs}ms`,
  )
  console.log(`Phrase: "${phrase}"`)
  console.log(
    `Record: format=${format} bitrateBps=${recorderOptions.opusBitrateBps ?? 256_000} maxSec=${(recorderOptions.maxDurationMs ?? 90_000) / 1000}`,
  )
  console.log(`Output path: ${outputPath}`)
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log('')

  const { agentOut, userInbound, userOut, agentInbound, cleanup } =
    await createBidirectionalLoopback()

  const recorder = new SessionRecorder(recorderOptions)
  recorder.wrapOutboundTrack(agentOut as LocalAudioTrack)
  recorder.wrapInboundTrack(userInbound)

  const speaker = new VoiceAgent({
    tts: config.tts,
    events: { mode: 'stream' },
    vad: { enabled: false },
  })
  const listener = new VoiceAgent({
    stt: config.stt,
    events: { mode: 'stream' },
    vad: config.vad,
  })

  await speaker.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await listener.attach({ inboundTrack: userInbound, outboundTrack: userOut })
  await speaker.start()
  await listener.start()

  const warmupS = Number(process.env.SHERPA_ROUNDTRIP_WARMUP_S ?? DEFAULT_WARMUP_S)
  await streamSilence(agentOut, warmupS)

  const pumpStarted = { value: false }
  const collector = new ListenerUtteranceCollector(listener, pumpStarted, verbose)
  collector.startPump()
  const agentEndLatch = new AgentSpeakingEndLatch()
  startSpeakerSpeechPump(speaker, agentEndLatch)

  console.log('[speaker] Synthesizing phrase with recording…')
  const playbackPromise = playSpeakerTtsWithPostSilence({
    speaker,
    speakerOut: agentOut,
    phrase,
    postTtsSilenceS,
    playbackTimeoutMs: DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
    agentSpeakingEndLatch: agentEndLatch,
  })
  const recognizedPromise = collector.waitForNextAfterPlayback(
    playbackPromise,
    timeoutMs,
    finalizeWaitMs,
  )
  await playbackPromise
  const recognized = await recognizedPromise

  const evaluation = evaluateCountingRoundtrip({
    phrase,
    recognized,
    stats: collector.stats,
    minNumberWords,
    numberWords: NUMBER_WORDS_ONE_TO_TEN,
    label: 'listener',
  })

  await listener.stop().catch(() => undefined)
  await speaker.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  mkdirSync(dirname(outputPath), { recursive: true })
  let recordResult
  try {
    recordResult = recorder.finalize(outputPath)
  } catch (error) {
    exitSherpaRoundtripFailure({
      reason: 'session recorder finalize failed',
      error,
      legs: [
        {
          label: 'listener',
          phrase,
          recognized: evaluation.recognized,
          stats: evaluation.stats,
        },
      ],
    })
  }

  const absOut = resolve(outputPath)
  let channelEval = { passed: true, warnings: [] as string[] }
  if (format === 'wav') {
    const { readFileSync } = await import('node:fs')
    channelEval = evaluateRecordedWavChannels({
      wav: readFileSync(outputPath),
      outboundFrames: recordResult.outboundFrames,
      inboundFrames: recordResult.inboundFrames,
    })
  } else if (recordResult.outboundFrames === 0 || recordResult.inboundFrames === 0) {
    channelEval = {
      passed: false,
      warnings: [
        `frame counts L=${recordResult.outboundFrames} R=${recordResult.inboundFrames} (soft-assert for opus)`,
      ],
    }
  }

  console.log('')
  console.log('=== Results ===')
  console.log(`Recognized: "${evaluation.recognized}"`)
  console.log(
    `Record: format=${recordResult.format} duration_ms=${recordResult.durationMs} out_frames=${recordResult.outboundFrames} in_frames=${recordResult.inboundFrames}`,
  )
  if (format === 'opus') {
    console.log(`LISTEN_BACK_OPUS=${absOut}`)
  } else {
    console.log(`LISTEN_BACK_WAV=${absOut}`)
  }
  console.log('Channels: L=speaker outbound (TTS)  R=listener inbound (received audio)')

  if (channelEval.warnings.length > 0) {
    console.warn('[record] channel warnings:')
    for (const w of channelEval.warnings) console.warn(`  - ${w}`)
  }

  if (!evaluation.passed) {
    console.warn('[record] STT assertions failed but recording was written for listen-back')
    exitSherpaRoundtripFailure({
      reason: 'counting leg assertions failed (recording written)',
      failures: evaluation.failures,
      legs: [
        {
          label: 'listener',
          phrase,
          recognized: evaluation.recognized,
          stats: evaluation.stats,
        },
      ],
    })
  }

  if (!channelEval.passed) {
    console.warn('[record] channel soft-assert failed — file still available for listen-back')
  }

  console.log('\nSherpa record roundtrip OK.')
  process.exit(0)
}

const isMain = process.argv[1]?.endsWith('roundtrip-record.ts') === true

if (isMain) {
  main().catch((error: unknown) => {
    exitSherpaRoundtripFailure({
      reason: 'uncaught error',
      error,
    })
  })
}
