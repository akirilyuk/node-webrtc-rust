/**
 * Sherpa roundtrip — `user_speaking_end` must arrive immediately before `user_speech_final`.
 *
 * Regression: emitting speaking_end before STT finalize completes left an ~8s gap in the
 * multi-client UI while Sherpa still decoded the utterance with the gate closed.
 *
 * Run:
 *   npm run start:roundtrip-utterance-timing --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 */

import { VoiceAgent, VOICE_AGENT_VAD_PRESET } from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { streamSilence } from './pcm-relay.js'
import { resolveVoiceConfig } from './resolve-voice-config.js'
import {
  DEFAULT_COUNTING_PHRASE_ONE_TO_TEN,
  DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS,
  evaluateSpeakingEndFinalTiming,
  installRoundtripWallClockTimeout,
  ListenerUtteranceCollector,
  postTtsSilenceSeconds,
  sttFinalizeWaitMs,
} from './roundtrip-counting.js'
import { playTtsAndCollect } from './roundtrip-counting-echo.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'

const DEFAULT_TIMEOUT_MS = 45_000
const DEFAULT_WARMUP_S = 0.6

async function main(): Promise<void> {
  installRoundtripWallClockTimeout(50_000)

  const phrase =
    process.env.SHERPA_UTTERANCE_TIMING_PHRASE?.trim() || DEFAULT_COUNTING_PHRASE_ONE_TO_TEN
  const maxGapMs = Number(
    process.env.SHERPA_MAX_SPEAKING_END_TO_FINAL_MS ?? DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS,
  )

  const { config, label, sttModelPath, ttsModelPath } = resolveVoiceConfig()
  const timeoutMs = Number(process.env.SHERPA_COUNTING_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
  const finalizeWaitMs = sttFinalizeWaitMs(config)
  const postTtsSilenceS = postTtsSilenceSeconds(config)
  const verbose = process.env.SHERPA_COUNTING_VERBOSE === '1'

  console.log('=== Sherpa utterance timing roundtrip ===')
  console.log(`Pipeline: ${label}`)
  console.log(
    `Listener: gateStt=${config.vad?.gateStt !== false}  sttGateHold=${config.vad?.sttGateHoldMs ?? VOICE_AGENT_VAD_PRESET.sttGateHoldMs}ms`,
  )
  console.log(`Phrase: "${phrase}"`)
  console.log(`Assert: user_speaking_end → user_speech_final within ${maxGapMs} ms`)
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

  const collector = new ListenerUtteranceCollector(listener, { value: false }, verbose)
  collector.startPump()

  const recognized = await playTtsAndCollect({
    speaker,
    speakerOut: agentOut,
    listenerCollector: collector,
    text: phrase,
    postTtsSilenceS,
    timeoutMs,
    finalizeWaitMs,
    logLabel: 'utterance timing',
  })

  const timing = evaluateSpeakingEndFinalTiming({
    stats: collector.stats,
    maxGapMs,
    label: 'listener',
  })

  console.log(`Recognized: "${recognized}"`)
  console.log(
    `Timing: speaking_end=${collector.stats.speakingEndAtMs ?? 'n/a'}  final=${collector.stats.speechFinalAtMs ?? 'n/a'}  gap=${timing.gapMs ?? 'n/a'} ms`,
  )

  await speaker.stop().catch(() => undefined)
  await listener.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  if (!timing.passed) {
    exitSherpaRoundtripFailure({
      reason: 'speaking_end / final timing assertions failed',
      failures: timing.failures,
      legs: [
        {
          label: 'listener',
          recognized,
          stats: collector.stats,
        },
      ],
    })
  }

  console.log('\nUtterance timing roundtrip OK — speaking_end and final are paired.')
  process.exit(0)
}

const isMain = process.argv[1]?.endsWith('roundtrip-utterance-timing.ts') === true

if (isMain) {
  main().catch((error: unknown) => {
    exitSherpaRoundtripFailure({
      reason: 'uncaught error',
      error,
    })
  })
}
