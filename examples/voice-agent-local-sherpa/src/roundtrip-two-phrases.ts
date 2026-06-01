/**
 * Sherpa roundtrip — two separate phrases → two `user_speech_final` events.
 *
 * Mirrors multi-client usage: count 1–10 (turn 1), pause, then a second sentence (turn 2).
 * Each turn must produce its own final with paired `user_speaking_end` (no merge, no orphan end).
 *
 * Run:
 *   npm run start:roundtrip-two-phrases --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 */

import type { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import { VoiceAgent, VOICE_AGENT_VAD_PRESET } from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { streamSilence } from './pcm-relay.js'
import { resolveVoiceConfig } from './resolve-voice-config.js'
import {
  DEFAULT_COUNTING_PHRASE_ONE_TO_TEN,
  DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS,
  evaluateFinalSequence,
  FinalSequenceCollector,
  interPhraseSilenceSeconds,
  postTtsSilenceSeconds,
} from './roundtrip-counting.js'

const DEFAULT_PHRASE_TWO = 'I am done speaking'
const DEFAULT_TIMEOUT_MS = 120_000
const DEFAULT_MIN_NUMBER_WORDS = 8
const DEFAULT_WARMUP_S = 0.6

async function speakPhrase(params: {
  speaker: VoiceAgent
  speakerOut: LocalAudioTrack
  text: string
  postTtsSilenceS: number
  logLabel: string
}): Promise<void> {
  console.log(`[${params.logLabel}] TTS: "${params.text}"`)
  await params.speaker.sendTextToTTS(params.text)
  await streamSilence(params.speakerOut, params.postTtsSilenceS)
}

async function main(): Promise<void> {
  const phrase1 =
    process.env.SHERPA_TWO_PHRASE_FIRST?.trim() || DEFAULT_COUNTING_PHRASE_ONE_TO_TEN
  const phrase2 = process.env.SHERPA_TWO_PHRASE_SECOND?.trim() || DEFAULT_PHRASE_TWO
  const maxGapMs = Number(
    process.env.SHERPA_MAX_SPEAKING_END_TO_FINAL_MS ?? DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS,
  )

  const { config, label, sttModelPath, ttsModelPath } = resolveVoiceConfig()
  const timeoutMs = Number(process.env.SHERPA_COUNTING_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
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
    `Timing: postTts=${postTtsSilenceS.toFixed(1)}s  betweenPhrases=${betweenPhrasesS.toFixed(1)}s  maxEnd→Final=${maxGapMs}ms`,
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
  await Promise.all([streamSilence(agentOut, warmupS), streamSilence(userOut, warmupS)])

  const collector = new FinalSequenceCollector(listener, { value: false }, verbose)
  collector.startPump()

  const wait1 = collector.waitForFinalCount(1, timeoutMs)
  await speakPhrase({
    speaker,
    speakerOut: agentOut,
    text: phrase1,
    postTtsSilenceS,
    logLabel: 'phrase 1',
  })
  await wait1
  console.log(`✓ Final 1: "${collector.records[0]?.text ?? ''}"`)

  console.log(`Between phrases: ${betweenPhrasesS.toFixed(1)}s silence (end turn 1 before turn 2)`)
  await streamSilence(agentOut, betweenPhrasesS)
  await streamSilence(userOut, betweenPhrasesS)

  const wait2 = collector.waitForFinalCount(2, timeoutMs)
  await speakPhrase({
    speaker,
    speakerOut: agentOut,
    text: phrase2,
    postTtsSilenceS,
    logLabel: 'phrase 2',
  })
  await wait2
  console.log(`✓ Final 2: "${collector.records[1]?.text ?? ''}"`)

  const evaluation = evaluateFinalSequence({
    records: collector.records,
    expectedCount: 2,
    maxGapMs,
    minNumberWordsFirst: minNumberWords,
    textIncludes: ['', 'done'],
    label: 'two-phrase',
  })

  await speaker.stop().catch(() => undefined)
  await listener.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  if (!evaluation.passed) {
    console.error('\nTwo-phrase roundtrip FAILED:')
    for (const msg of evaluation.failures) console.error(`  - ${msg}`)
    process.exit(1)
  }

  console.log('\nTwo-phrase roundtrip OK — 2 finals, each with paired speaking_end.')
  console.log(
    '(In multi-client, each final triggers voice-handler → You said: … — 2 replies expected.)',
  )
  process.exit(0)
}

const isMain = process.argv[1]?.endsWith('roundtrip-two-phrases.ts') === true

if (isMain) {
  main().catch((error: unknown) => {
    console.error(error)
    process.exit(1)
  })
}
