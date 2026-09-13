/**
 * Sherpa bidirectional echo + language ID — staging echo-smoke order.
 *
 * Agent 1 speaks counting; Agent 2 (echo) enables Whisper LID at cloud-default minSpeechMs and
 * **immediately** `sendTextToTTS("Okay. " + text)` on `user_speech_final` (no harness settle gap).
 * Agent 1 inbound STT must hear a non-digit prefix before the first number word — same class as
 * e2e `local STT missing any prefix before counting`.
 *
 *   npm run start:roundtrip-counting-echo-lid --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 */

import {
  VoiceAgent,
  VOICE_AGENT_VAD_PRESET,
  SPEECH_EVENT_TYPE,
  type VoiceAgentConfig,
} from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { streamSilence } from './pcm-relay.js'
import { resolveRoundtripVoiceConfig, withRoundtripHarnessSilence } from './resolve-voice-config.js'
import {
  DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
  DEFAULT_COUNTING_PHRASE_ONE_TO_TEN,
  AgentSpeakingEndLatch,
  ListenerUtteranceCollector,
  evaluateCountingRoundtrip,
  installRoundtripWallClockTimeout,
  playSpeakerTtsWithPostSilence,
  postTtsSilenceSeconds,
  sttFinalizeWaitMs,
  waitAgentPlaybackEndRace,
} from './roundtrip-counting.js'
import { echoVadConfig } from './roundtrip-counting-echo.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import { logRoundtripSpeechEvent } from './roundtrip-speech-events.js'
import { resolveLidModelPath } from './roundtrip-language-id.js'
import {
  buildEchoLanguageIdConfig,
  formatSherpaLidTtsExclusionLabel,
  parseSherpaLidTtsExclusion,
} from './roundtrip-counting-echo-lid-env.js'
import {
  CLOUD_DEFAULT_LID_MIN_SPEECH_MS,
  ECHO_SMOKE_REPLY_PREFIX,
  evaluateEchoLidInboundTranscript,
  formatEchoSmokeReply,
} from './roundtrip-counting-echo-lid-prefix.js'

const DEFAULT_TIMEOUT_MS = 90_000
const DEFAULT_WARMUP_S = 0.6
const DEFAULT_MIN_NUMBER_WORDS = 8

export {
  CLOUD_DEFAULT_LID_MIN_SPEECH_MS,
  ECHO_SMOKE_REPLY_PREFIX,
  evaluateEchoLidInboundTranscript,
  formatEchoSmokeReply,
  transcriptHasPrefixBeforeCounting,
} from './roundtrip-counting-echo-lid-prefix.js'

function agent2ConfigWithLanguageId(
  base: VoiceAgentConfig,
  lidModelPath: string,
  ttsExclusion?: boolean,
): VoiceAgentConfig {
  return withRoundtripHarnessSilence({
    ...base,
    languageId: buildEchoLanguageIdConfig(lidModelPath, ttsExclusion),
    events: { mode: 'stream' },
    vad: echoVadConfig(base),
  })
}

function agent1Config(base: VoiceAgentConfig): VoiceAgentConfig {
  return withRoundtripHarnessSilence({
    ...base,
    events: { mode: 'stream' },
    vad: echoVadConfig(base),
  })
}

/** Fire echo TTS on every final — do not wait for LID settle or inter-leg silence. */
async function pumpImmediateEchoOnFinal(
  agent2: VoiceAgent,
  agent2EndLatch: AgentSpeakingEndLatch,
): Promise<void> {
  try {
    for await (const event of agent2.speechEvents()) {
      logRoundtripSpeechEvent('agent2-echo', event)
      agent2EndLatch.observe(event)
      if (event.type !== SPEECH_EVENT_TYPE.userSpeechFinal) {
        continue
      }
      const text = event.text?.trim()
      if (!text) {
        continue
      }
      const echoText = formatEchoSmokeReply(text)
      console.log(
        `[agent2] immediate echo on user_speech_final: "${echoText.slice(0, 96)}${echoText.length > 96 ? '…' : ''}"`,
      )
      void agent2.sendTextToTTS(echoText).catch((error: unknown) => {
        console.error('[agent2] sendTextToTTS failed:', error)
      })
    }
  } catch (error) {
    console.error('[agent2] echo pump ended:', error)
  }
}

async function main(): Promise<void> {
  installRoundtripWallClockTimeout(180_000)

  const countingPhrase =
    process.env.SHERPA_COUNTING_PHRASE?.trim() || DEFAULT_COUNTING_PHRASE_ONE_TO_TEN
  const { config: base, label, sttModelPath, ttsModelPath } = resolveRoundtripVoiceConfig()
  const lidModelPath = resolveLidModelPath()
  const lidTtsExclusion = parseSherpaLidTtsExclusion()
  const timeoutMs = Number(process.env.SHERPA_COUNTING_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
  const minNumberWords = Number(
    process.env.SHERPA_COUNTING_MIN_NUMBER_WORDS ?? DEFAULT_MIN_NUMBER_WORDS,
  )
  const finalizeWaitMs = sttFinalizeWaitMs(base)
  const postTtsSilenceS = postTtsSilenceSeconds(base)
  const verbose = process.env.SHERPA_COUNTING_VERBOSE === '1'

  console.log('=== Sherpa counting echo + LID (staging echo-smoke order) ===')
  console.log(`Pipeline: ${label}`)
  console.log(
    `VAD: gateStt=${base.vad?.gateStt !== false}  minSilence=${base.vad?.minSilenceDurationMs ?? VOICE_AGENT_VAD_PRESET.minSilenceDurationMs}ms`,
  )
  console.log(`STT: ${sttModelPath}`)
  console.log(`TTS: ${ttsModelPath}`)
  console.log(`LID: ${lidModelPath}  minSpeechMs=${CLOUD_DEFAULT_LID_MIN_SPEECH_MS}`)
  console.log(`SHERPA_LID_TTS_EXCLUSION=${formatSherpaLidTtsExclusionLabel(lidTtsExclusion)}`)
  console.log(`Echo prefix: "${ECHO_SMOKE_REPLY_PREFIX}"`)
  console.log(`Agent1 speaks: "${countingPhrase}"`)
  console.log(`Timing: postTtsSilence=${postTtsSilenceS.toFixed(1)}s  timeout=${timeoutMs}ms`)
  console.log('')

  const { agentOut, userInbound, userOut, agentInbound, cleanup } =
    await createBidirectionalLoopback()

  const agent1 = new VoiceAgent(agent1Config(base))
  const agent2 = new VoiceAgent(agent2ConfigWithLanguageId(base, lidModelPath, lidTtsExclusion))

  await agent1.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await agent2.attach({ inboundTrack: userInbound, outboundTrack: userOut })
  await agent1.start()
  await agent2.start()

  const warmupS = Number(process.env.SHERPA_ROUNDTRIP_WARMUP_S ?? DEFAULT_WARMUP_S)
  await Promise.all([streamSilence(agentOut, warmupS), streamSilence(userOut, warmupS)])

  const agent1EndLatch = new AgentSpeakingEndLatch()
  const agent2EndLatch = new AgentSpeakingEndLatch()
  const collectorAgent1 = new ListenerUtteranceCollector(
    agent1,
    { value: false },
    verbose,
    'agent1',
    agent1EndLatch,
  )
  collectorAgent1.startPump()
  const echoPumpTask = pumpImmediateEchoOnFinal(agent2, agent2EndLatch)

  const echoEndBaseline = agent2EndLatch.endEventsSeen()

  console.log(`Agent1 TTS: "${countingPhrase}"`)
  const agent1PlaybackPromise = playSpeakerTtsWithPostSilence({
    speaker: agent1,
    speakerOut: agentOut,
    phrase: countingPhrase,
    postTtsSilenceS,
    playbackTimeoutMs: DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
    agentSpeakingEndLatch: agent1EndLatch,
  })

  const playbackAndEchoDone = agent1PlaybackPromise.then(async () => {
    await waitAgentPlaybackEndRace({
      phrase: formatEchoSmokeReply(countingPhrase),
      capMs: timeoutMs,
      waitForAgentSpeakingEnd: () => agent2EndLatch.waitAfterCount(echoEndBaseline, timeoutMs),
    })
    if (postTtsSilenceS > 0) {
      console.log(`[agent2] post-TTS silence ${postTtsSilenceS.toFixed(1)}s on userOut (echo leg)`)
      await streamSilence(userOut, postTtsSilenceS)
    }
  })

  const recognized = await collectorAgent1.waitForNextAfterPlayback(
    playbackAndEchoDone,
    timeoutMs,
    finalizeWaitMs,
  )
  await playbackAndEchoDone

  const best = collectorAgent1.stats.finals.reduce(
    (a, b) => (a.trim().length >= b.trim().length ? a : b),
    recognized,
  )
  const inboundTranscript = best.trim().length > recognized.trim().length ? best.trim() : recognized

  console.log('')
  console.log(`Agent1 inbound recognized: "${inboundTranscript}"`)

  const prefixEval = evaluateEchoLidInboundTranscript({
    recognized: inboundTranscript,
    spokenCountingPhrase: countingPhrase,
    minNumberWords,
  })
  const countingEval = evaluateCountingRoundtrip({
    phrase: countingPhrase,
    recognized: inboundTranscript,
    stats: collectorAgent1.stats,
    minNumberWords,
    label: 'Agent1 inbound echo',
  })

  const failures = [...prefixEval.failures, ...countingEval.failures]

  await agent1.stop().catch(() => undefined)
  await agent2.stop().catch(() => undefined)
  await echoPumpTask.catch(() => undefined)
  await cleanup().catch(() => undefined)

  if (failures.length > 0) {
    exitSherpaRoundtripFailure({
      reason: 'counting echo + LID assertions failed',
      failures,
      legs: [
        {
          label: 'Agent1 inbound (echo with LID overlap check)',
          phrase: countingPhrase,
          recognized: inboundTranscript,
          stats: collectorAgent1.stats,
        },
      ],
    })
  }

  console.log(
    `\nCounting echo + LID roundtrip OK — Agent1 heard reply prefix before counting digits (SHERPA_LID_TTS_EXCLUSION=${formatSherpaLidTtsExclusionLabel(lidTtsExclusion)}).`,
  )
  process.exit(0)
}

const isMain = process.argv[1]?.endsWith('roundtrip-counting-echo-lid.ts') === true

if (isMain) {
  main().catch((error: unknown) => {
    exitSherpaRoundtripFailure({
      reason: 'uncaught error',
      error,
    })
  })
}
