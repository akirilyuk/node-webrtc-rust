/**
 * Sherpa TTS stream-chunks roundtrip — real models, TTS → WebRTC → STT.
 *
 * Compares legacy buffered synth (`VOICE_TTS_STREAM_CHUNKS=0`) vs progressive
 * streaming (default `1`) for time-to-first outbound audio (`agent_speaking_start`),
 * and asserts STT still recognizes the phrase on both paths.
 *
 * Run:
 *   npm run start:roundtrip-tts-stream --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 */

import type { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import {
  VoiceAgent,
  VOICE_AGENT_VAD_PRESET,
  SPEECH_EVENT_TYPE,
  type SpeechEvent,
} from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { streamSilence } from './pcm-relay.js'
import { resolveRoundtripVoiceConfig } from './resolve-voice-config.js'
import {
  AgentSpeakingEndLatch,
  installRoundtripWallClockTimeout,
  ListenerUtteranceCollector,
  normalizeForCompare,
  postTtsSilenceSeconds,
  roundtripWallClockMs,
  startSpeakerSpeechPump,
  sttFinalizeWaitMs,
  wordSimilarity,
} from './roundtrip-counting.js'
import { playTtsAndCollect } from './roundtrip-counting-echo.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import {
  evaluateRecognizedContainsWords,
  evaluateStreamingFasterOrEqual,
} from './roundtrip-tts-stream-helpers.js'

const DEFAULT_TIMEOUT_MS = 60_000
const DEFAULT_WARMUP_S = 0.6
/** Streaming must not regress STT quality vs buffered (was ~0.70 when sentence deltas were sliced wrong). */
const DEFAULT_MIN_SIMILARITY = 0.9
/** Long enough that ONNX generate wall time is measurable vs first callback. */
const DEFAULT_PHRASE =
  'Why do you need so much time until you start speaking clearly to the listener. ' +
  'Please explain every step carefully so that anyone following along can understand ' +
  'the complete answer without needing to ask for clarification again.'

type StreamMode = 'streaming' | 'buffered'

function applyStreamMode(mode: StreamMode): void {
  // Read per TTS job in Rust — safe to toggle between utterances on a live agent.
  process.env.VOICE_TTS_STREAM_CHUNKS = mode === 'streaming' ? '1' : '0'
  // Fair latency compare: do not serve the second mode from phrase cache.
  process.env.SHERPA_TTS_PHRASE_CACHE = '0'
}

async function runLeg(params: {
  mode: StreamMode
  speaker: VoiceAgent
  speakerOut: LocalAudioTrack
  listenerCollector: ListenerUtteranceCollector
  agentSpeakingEndLatch: AgentSpeakingEndLatch
  phrase: string
  postTtsSilenceS: number
  timeoutMs: number
  finalizeWaitMs: number
}): Promise<{ firstAudioMs: number; recognized: string }> {
  applyStreamMode(params.mode)
  params.listenerCollector.resetStatsForUtterance()

  let firstAudioMs: number | null = null
  const t0 = performance.now()
  const onStart = (_event: SpeechEvent): void => {
    if (firstAudioMs == null) {
      firstAudioMs = performance.now() - t0
    }
  }
  params.speaker.on(SPEECH_EVENT_TYPE.agentSpeakingStart, onStart)

  try {
    const recognized = await playTtsAndCollect({
      speaker: params.speaker,
      speakerOut: params.speakerOut,
      listenerCollector: params.listenerCollector,
      agentSpeakingEndLatch: params.agentSpeakingEndLatch,
      text: params.phrase,
      postTtsSilenceS: params.postTtsSilenceS,
      timeoutMs: params.timeoutMs,
      finalizeWaitMs: params.finalizeWaitMs,
      logLabel: `tts-stream ${params.mode}`,
    })
    if (firstAudioMs == null) {
      throw new Error(`${params.mode}: missing agent_speaking_start`)
    }
    return { firstAudioMs, recognized }
  } finally {
    params.speaker.off(SPEECH_EVENT_TYPE.agentSpeakingStart, onStart)
  }
}

async function main(): Promise<void> {
  const phrase = process.env.SHERPA_TTS_STREAM_PHRASE?.trim() || DEFAULT_PHRASE
  // Longer DEFAULT_PHRASE usually yields a clear first-audio win; override if flaky.
  const minImprovementMs = Number(process.env.SHERPA_TTS_STREAM_MIN_IMPROVEMENT_MS ?? 40)
  const maxRegressionMs = Number(process.env.SHERPA_TTS_STREAM_MAX_REGRESSION_MS ?? 80)
  const minSimilarity = Number(process.env.SHERPA_TTS_STREAM_MIN_SIMILARITY ?? DEFAULT_MIN_SIMILARITY)

  const { config, label, sttModelPath, ttsModelPath } = resolveRoundtripVoiceConfig()
  // Two long TTS→STT legs + warmup — use the long profile wall budget.
  installRoundtripWallClockTimeout(roundtripWallClockMs(config, 'long'))
  const timeoutMs = Number(process.env.SHERPA_COUNTING_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
  const finalizeWaitMs = sttFinalizeWaitMs(config)
  const postTtsSilenceS = postTtsSilenceSeconds(config)
  const verbose = process.env.SHERPA_COUNTING_VERBOSE === '1'

  console.log('=== Sherpa TTS stream-chunks roundtrip (TTS → STT) ===')
  console.log(`Pipeline: ${label}`)
  console.log(`Phrase: "${phrase}"`)
  console.log(
    `Assert: streaming first-audio ≤ buffered - ${minImprovementMs} + ${maxRegressionMs} ms jitter`,
  )
  console.log(`Assert: STT similarity ≥ ${minSimilarity} on both modes`)
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log('')

  // Disable cache for the whole process before agents start.
  process.env.SHERPA_TTS_PHRASE_CACHE = '0'

  const { agentOut, userInbound, userOut, agentInbound, cleanup } =
    await createBidirectionalLoopback()

  const speaker = new VoiceAgent({
    tts: config.tts,
    // `both`: `on()` for first-audio timing + `speechEvents()` for AgentSpeakingEndLatch.
    events: { mode: 'both' },
    vad: { enabled: false },
  })
  const listener = new VoiceAgent({
    stt: config.stt,
    events: { mode: 'stream' },
    vad: {
      ...VOICE_AGENT_VAD_PRESET,
      ...config.vad,
      bargeIn: { ...VOICE_AGENT_VAD_PRESET.bargeIn, ...config.vad?.bargeIn, enabled: false },
    },
  })

  await speaker.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await listener.attach({ inboundTrack: userInbound, outboundTrack: userOut })
  await speaker.start()
  await listener.start()

  const warmupS = Number(process.env.SHERPA_ROUNDTRIP_WARMUP_S ?? DEFAULT_WARMUP_S)
  await Promise.all([streamSilence(agentOut, warmupS), streamSilence(userOut, warmupS)])

  const collector = new ListenerUtteranceCollector(listener, { value: false }, verbose)
  collector.startPump()
  const agentEndLatch = new AgentSpeakingEndLatch()
  startSpeakerSpeechPump(speaker, agentEndLatch)

  // Warm engine so cold start does not dominate the buffered sample.
  applyStreamMode('buffered')
  await playTtsAndCollect({
    speaker,
    speakerOut: agentOut,
    listenerCollector: collector,
    agentSpeakingEndLatch: agentEndLatch,
    text: 'Warm up.',
    postTtsSilenceS,
    timeoutMs,
    finalizeWaitMs,
    logLabel: 'tts-stream warmup',
  })
  collector.resetStatsForUtterance()
  await streamSilence(agentOut, 0.4)

  const buffered = await runLeg({
    mode: 'buffered',
    speaker,
    speakerOut: agentOut,
    listenerCollector: collector,
    agentSpeakingEndLatch: agentEndLatch,
    phrase,
    postTtsSilenceS,
    timeoutMs,
    finalizeWaitMs,
  })
  console.log(
    `buffered: firstAudio=${buffered.firstAudioMs.toFixed(0)}ms  recognized="${buffered.recognized}"`,
  )

  await streamSilence(agentOut, 0.5)

  const streaming = await runLeg({
    mode: 'streaming',
    speaker,
    speakerOut: agentOut,
    listenerCollector: collector,
    agentSpeakingEndLatch: agentEndLatch,
    phrase,
    postTtsSilenceS,
    timeoutMs,
    finalizeWaitMs,
  })
  console.log(
    `streaming: firstAudio=${streaming.firstAudioMs.toFixed(0)}ms  recognized="${streaming.recognized}"`,
  )

  await speaker.stop().catch(() => undefined)
  await listener.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  const latency = evaluateStreamingFasterOrEqual({
    streamingMs: streaming.firstAudioMs,
    bufferedMs: buffered.firstAudioMs,
    minImprovementMs,
    maxRegressionMs,
  })

  const requiredWords = ['why', 'need', 'time', 'speaking']
  const sttBuffered = evaluateRecognizedContainsWords({
    recognized: buffered.recognized,
    requiredWords,
    label: 'buffered',
  })
  const sttStreaming = evaluateRecognizedContainsWords({
    recognized: streaming.recognized,
    requiredWords,
    label: 'streaming',
  })

  const simBuffered = wordSimilarity(
    normalizeForCompare(phrase),
    normalizeForCompare(buffered.recognized),
  )
  const simStreaming = wordSimilarity(
    normalizeForCompare(phrase),
    normalizeForCompare(streaming.recognized),
  )

  const failures = [...latency.failures, ...sttBuffered.failures, ...sttStreaming.failures]
  if (simBuffered < minSimilarity) {
    failures.push(`buffered STT similarity ${simBuffered.toFixed(2)} < ${minSimilarity}`)
  }
  if (simStreaming < minSimilarity) {
    failures.push(`streaming STT similarity ${simStreaming.toFixed(2)} < ${minSimilarity}`)
  }

  console.log('')
  const deltaMs = buffered.firstAudioMs - streaming.firstAudioMs
  console.log(
    `Latency: streaming=${streaming.firstAudioMs.toFixed(0)}ms buffered=${buffered.firstAudioMs.toFixed(0)}ms ` +
      `(delta=${deltaMs.toFixed(0)}ms, minImprovement=${minImprovementMs}, maxRegression=${maxRegressionMs})`,
  )
  console.log(
    `STT similarity: buffered=${simBuffered.toFixed(2)} streaming=${simStreaming.toFixed(2)}`,
  )

  if (failures.length > 0) {
    exitSherpaRoundtripFailure({
      reason: failures.join('; '),
      failures,
      legs: [
        {
          label: 'buffered',
          phrase,
          recognized: buffered.recognized,
        },
        {
          label: 'streaming',
          phrase,
          recognized: streaming.recognized,
        },
      ],
    })
  }

  console.log('OK — streaming first-audio faster; STT OK on both paths')
  process.exit(0)
}

main().catch((err) => {
  exitSherpaRoundtripFailure({
    reason: err instanceof Error ? err.message : String(err),
    error: err,
  })
})
