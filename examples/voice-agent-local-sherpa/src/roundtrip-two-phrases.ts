/**
 * Sherpa roundtrip — two separate phrases → two `user_speech_final` events.
 *
 * Mirrors multi-client usage: count 1–10 (turn 1), pause, then a second sentence (turn 2).
 * Each turn must produce its own final with paired `user_speaking_end` (no merge, no orphan end).
 *
 * Run:
 *   npm run start:roundtrip-two-phrases --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Env:
 *   SHERPA_ROUNDTRIP_WALL_MS           hard process exit (default 70_000)
 *   SHERPA_COUNTING_TIMEOUT_MS         per-phrase STT wait (default 30_000)
 */

import type { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import { VoiceAgent, VOICE_AGENT_VAD_PRESET } from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { streamSilence } from './pcm-relay.js'
import { resolveRoundtripVoiceConfig } from './resolve-voice-config.js'
import {
  DEFAULT_COUNTING_PHRASE_ONE_TO_TEN,
  DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS,
  DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
  evaluateFinalSequence,
  finalRecordFromStats,
  installRoundtripWallClockTimeout,
  interPhraseSilenceSeconds,
  ListenerUtteranceCollector,
  AgentSpeakingEndLatch,
  playSpeakerTtsWithPostSilence,
  postTtsSilenceSeconds,
  startSpeakerSpeechPump,
  sttFinalizeWaitMs,
} from './roundtrip-counting.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import { evaluateNormalUtteranceLifecycle } from './roundtrip-stt-lifecycle-helpers.js'

const DEFAULT_PHRASE_TWO = 'I am done speaking'
/** Per-phrase STT wait — keep low; wall clock is the backstop. */
const DEFAULT_TIMEOUT_MS = 30_000
const DEFAULT_MIN_NUMBER_WORDS = 8
const DEFAULT_WARMUP_S = 0.6

async function speakPhrase(params: {
  speaker: VoiceAgent
  speakerOut: LocalAudioTrack
  agentEndLatch: AgentSpeakingEndLatch
  text: string
  postTtsSilenceS: number
  logLabel: string
}): Promise<void> {
  console.log(`[${params.logLabel}] TTS: "${params.text}"`)
  await playSpeakerTtsWithPostSilence({
    speaker: params.speaker,
    speakerOut: params.speakerOut,
    phrase: params.text,
    postTtsSilenceS: params.postTtsSilenceS,
    playbackTimeoutMs: DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
    agentSpeakingEndLatch: params.agentEndLatch,
  })
}

async function main(): Promise<void> {
  installRoundtripWallClockTimeout(70_000)

  const phrase1 = process.env.SHERPA_TWO_PHRASE_FIRST?.trim() || DEFAULT_COUNTING_PHRASE_ONE_TO_TEN
  const phrase2 = process.env.SHERPA_TWO_PHRASE_SECOND?.trim() || DEFAULT_PHRASE_TWO
  const maxGapMs = Number(
    process.env.SHERPA_MAX_SPEAKING_END_TO_FINAL_MS ?? DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS,
  )

  const { config, label, sttModelPath, ttsModelPath } = resolveRoundtripVoiceConfig()
  const timeoutMs = Number(process.env.SHERPA_COUNTING_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
  const finalizeWaitMs = sttFinalizeWaitMs(config)
  const postTtsSilenceS = postTtsSilenceSeconds(config)
  const betweenPhrasesS = interPhraseSilenceSeconds(config)
  const minNumberWords = Number(
    process.env.SHERPA_COUNTING_MIN_NUMBER_WORDS ?? DEFAULT_MIN_NUMBER_WORDS,
  )
  const verbose = process.env.SHERPA_COUNTING_VERBOSE === '1'

  console.log('=== Sherpa two-phrase roundtrip (2× user_speech_final) ===')
  console.log(`Pipeline: ${label}`)
  console.log(
    `Listener: gateStt=${config.vad?.gateStt !== false}  sttGateHold=${config.vad?.sttGateHoldMs ?? VOICE_AGENT_VAD_PRESET.sttGateHoldMs}ms`,
  )
  console.log(`Phrase 1: "${phrase1}"`)
  console.log(`Phrase 2: "${phrase2}"`)
  console.log(
    `Timing: postTts=${postTtsSilenceS.toFixed(1)}s  betweenPhrases=${betweenPhrasesS.toFixed(1)}s  sttTimeout=${timeoutMs}ms  maxEnd→Final=${maxGapMs}ms`,
  )
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log('')

  const { agentOut, userInbound, userOut, agentInbound, cleanup } =
    await createBidirectionalLoopback()

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

  console.log('[phrase 1] waiting for user_speech_final…')
  collector.startEventRecording()
  const text1Promise = collector.waitForNext(timeoutMs, finalizeWaitMs)
  await speakPhrase({
    speaker,
    speakerOut: agentOut,
    agentEndLatch,
    text: phrase1,
    postTtsSilenceS,
    logLabel: 'phrase 1',
  })
  const text1 = await text1Promise
  console.log(`✓ Final 1: "${text1}"`)
  const records = [finalRecordFromStats(text1, collector.stats)]

  console.log(`Between phrases: ${betweenPhrasesS.toFixed(1)}s silence (end turn 1 before turn 2)`)
  await streamSilence(agentOut, betweenPhrasesS)
  await streamSilence(userOut, betweenPhrasesS)

  console.log('[phrase 2] waiting for user_speech_final…')
  const text2Promise = collector.waitForNext(timeoutMs, finalizeWaitMs)
  await speakPhrase({
    speaker,
    speakerOut: agentOut,
    agentEndLatch,
    text: phrase2,
    postTtsSilenceS,
    logLabel: 'phrase 2',
  })
  const text2 = await text2Promise
  console.log(`✓ Final 2: "${text2}"`)
  records.push(finalRecordFromStats(text2, collector.stats))

  const lifecycleEvents = collector.stopEventRecording()
  const lifecycleEval = evaluateNormalUtteranceLifecycle({
    events: lifecycleEvents,
    expectOpenCount: 2,
    label: 'two-phrase',
  })

  const evaluation = evaluateFinalSequence({
    records,
    expectedCount: 2,
    maxGapMs,
    minNumberWordsFirst: minNumberWords,
    textIncludes: ['', 'done'],
    label: 'two-phrase',
  })

  const failures = [...evaluation.failures]
  if (!lifecycleEval.passed) failures.push(...lifecycleEval.failures)

  await speaker.stop().catch(() => undefined)
  await listener.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  if (failures.length > 0) {
    exitSherpaRoundtripFailure({
      reason: 'two-phrase sequence assertions failed',
      failures,
      legs: [{ label: 'listener', stats: collector.stats }],
    })
  }

  console.log('\nTwo-phrase roundtrip OK — 2 finals, each with paired speaking_end.')
  process.exit(0)
}

const isMain = process.argv[1]?.endsWith('roundtrip-two-phrases.ts') === true

if (isMain) {
  main().catch((error: unknown) => {
    exitSherpaRoundtripFailure({
      reason: 'uncaught error',
      error,
    })
  })
}
