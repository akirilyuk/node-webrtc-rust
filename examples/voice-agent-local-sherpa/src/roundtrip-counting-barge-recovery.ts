/**
 * Sherpa echo roundtrip — barge-in during "You said: …" then recovery.
 *
 * Scenario (matches multi-client voice-handler):
 *   1. Agent1 counts 1–10 → Agent2 → Agent2 replies "You said: …" → Agent1 hears full echo.
 *   2. Agent1 counts again → Agent2 → Agent2 starts "You said: …" → **barge-in** mid-playback
 *      → Agent1 must hear only a **fraction** of the phrase.
 *   3. Agent1 speaks a short recovery phrase → full "You said: …" echo must work again.
 *
 * Barge-in is injected as tone on `agentOut` (loops to Agent2 inbound) while Agent2 TTS is playing.
 *
 * Run:
 *   npm run start:roundtrip-counting-barge-recovery --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 */

import type { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import {
  VoiceAgent,
  VOICE_AGENT_VAD_PRESET,
  type VoiceAgentConfig,
} from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { streamSilence } from './pcm-relay.js'
import { resolveVoiceConfig } from './resolve-voice-config.js'
import {
  countNumberWordsInTranscript,
  DEFAULT_COUNTING_PHRASE_ONE_TO_TEN,
  ListenerUtteranceCollector,
  NUMBER_WORDS_ONE_TO_TEN,
  postTtsSilenceSeconds,
  sttFinalizeWaitMs,
  DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
  installRoundtripWallClockTimeout,
  waitAgentPlaybackEndRace,
  wordSimilarity,
} from './roundtrip-counting.js'
import {
  evaluateCountingEchoLeg,
  formatAgent2EchoReply,
  playTtsAndCollect,
  runEchoRound,
  transcriptIncludesYouSaid,
} from './roundtrip-counting-echo.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'

/** Agent2 plays long echo TTS — STT-gated barge must interrupt mid-playback. */
function agent2VadConfig(base: VoiceAgentConfig): NonNullable<VoiceAgentConfig['vad']> {
  return {
    ...VOICE_AGENT_VAD_PRESET,
    ...base.vad,
    provider: 'energy',
    threshold: 0.05,
    gateStt: true,
    bargeIn: {
      ...VOICE_AGENT_VAD_PRESET.bargeIn,
      ...base.vad?.bargeIn,
      enabled: true,
      useVad: true,
      flushTts: true,
      requireSttPartial: true,
      agentPlaybackGuardMs: 0,
    },
  }
}

/** Agent1 injects user-side barge TTS on agentOut — must not self-barge on echo heard on agentInbound. */
function agent1VadConfig(base: VoiceAgentConfig): NonNullable<VoiceAgentConfig['vad']> {
  return {
    ...VOICE_AGENT_VAD_PRESET,
    ...base.vad,
    provider: 'energy',
    threshold: 0.05,
    gateStt: true,
    bargeIn: {
      ...VOICE_AGENT_VAD_PRESET.bargeIn,
      ...base.vad?.bargeIn,
      enabled: false,
      useVad: false,
      flushTts: false,
      requireSttPartial: false,
    },
  }
}

export const DEFAULT_RECOVERY_PHRASE = 'hello testing recovery one two three'

const DEFAULT_TIMEOUT_MS = 45_000
const DEFAULT_MIN_NUMBER_WORDS = 8
const DEFAULT_ECHO_MIN_WORDS = 8
const DEFAULT_MIN_ECHO_RETENTION = 0.6
const DEFAULT_WARMUP_S = 0.6
const DEFAULT_INTER_LEG_GAP_S = 0.5
const DEFAULT_INTER_ROUND_GAP_S = 1.0
/** Ms after agent2 TTS playback starts before barge TTS (mid-playback). */
const DEFAULT_BARGE_DELAY_MS = 700
const DEFAULT_BARGE_TONE_S = 1.5
/** Interrupted leg B must retain fewer number words than this. */
const DEFAULT_MAX_INTERRUPT_NUMBER_WORDS = 6
/** Interrupted leg B similarity vs full echo text must stay below this. */
const DEFAULT_MAX_INTERRUPT_SIMILARITY = 0.55

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

export interface InterruptedEchoLegResult {
  echoText: string
  recognized: string
  numberWordsFound: number
  similarity: number
  passed: boolean
  failures: string[]
}

/** Leg B was cut short by barge-in — transcript should be partial vs the full echo phrase. */
export function evaluateInterruptedEchoLeg(params: {
  echoText: string
  recognized: string
  maxNumberWords?: number
  maxSimilarity?: number
}): InterruptedEchoLegResult {
  const maxNumberWords = params.maxNumberWords ?? DEFAULT_MAX_INTERRUPT_NUMBER_WORDS
  const maxSimilarity = params.maxSimilarity ?? DEFAULT_MAX_INTERRUPT_SIMILARITY
  const recognized = params.recognized.trim()
  const failures: string[] = []

  const numberWordsFound = countNumberWordsInTranscript(recognized, NUMBER_WORDS_ONE_TO_TEN)
  const similarity = wordSimilarity(params.echoText, recognized)

  if (!recognized) {
    failures.push('interrupted leg B: recognized transcript is empty')
  }
  if (numberWordsFound > maxNumberWords) {
    failures.push(
      `interrupted leg B: expected at most ${maxNumberWords}/10 number words after barge-in, found ${numberWordsFound}`,
    )
  }
  if (similarity > maxSimilarity) {
    failures.push(
      `interrupted leg B: similarity ${(similarity * 100).toFixed(0)}% > ${(maxSimilarity * 100).toFixed(0)}% (playback should be truncated)`,
    )
  }

  return {
    echoText: params.echoText,
    recognized,
    numberWordsFound,
    similarity,
    passed: failures.length === 0,
    failures,
  }
}

/**
 * Agent2 speaks `You said: …` while Agent1 TTS on agentOut triggers STT-gated barge-in on Agent2.
 */
export async function playEchoLegBWithBargeIn(params: {
  agent1: VoiceAgent
  agent2: VoiceAgent
  agentOut: LocalAudioTrack
  userOut: LocalAudioTrack
  collectorAgent1: ListenerUtteranceCollector
  collectorAgent2: ListenerUtteranceCollector
  echoText: string
  bargeDelayMs: number
  bargeToneS: number
  postTtsSilenceS: number
  timeoutMs: number
  finalizeWaitMs: number
  logLabel: string
}): Promise<string> {
  console.log(
    `[${params.logLabel}] TTS (will barge after ${params.bargeDelayMs}ms): "${params.echoText.slice(0, 80)}${params.echoText.length > 80 ? '…' : ''}"`,
  )

  const recognizedPromise = params.collectorAgent1.waitForNext(
    params.timeoutMs,
    params.finalizeWaitMs,
  )

  const playbackDone = waitAgentPlaybackEndRace({
    phrase: params.echoText,
    capMs: DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
    waitForAgentSpeakingEnd: () =>
      params.collectorAgent1.waitForAgentSpeakingEnd(params.timeoutMs),
  })
  const speakingStarted = params.collectorAgent2.waitForAgentSpeakingStart(params.timeoutMs)
  const speakPromise = params.agent2.sendTextToTTS(params.echoText)

  await speakingStarted
  await sleep(params.bargeDelayMs)
  const bargePhrase = process.env.SHERPA_BARGE_RECOVERY_BARGE_PHRASE?.trim() || 'stop now please'
  console.log(
    `[${params.logLabel}] Barge via Sherpa TTS on agentOut: "${bargePhrase}" (${params.bargeToneS}s tail)…`,
  )
  const bargeSeen = params.collectorAgent2.waitForBargeIn(params.timeoutMs)
  await params.agent1.sendTextToTTS(bargePhrase)
  await streamSilence(params.agentOut, params.bargeToneS)
  await bargeSeen

  await speakPromise
  await playbackDone
  await streamSilence(params.userOut, params.postTtsSilenceS)
  return recognizedPromise
}

async function main(): Promise<void> {
  installRoundtripWallClockTimeout(240_000)

  const countingPhrase =
    process.env.SHERPA_COUNTING_PHRASE?.trim() || DEFAULT_COUNTING_PHRASE_ONE_TO_TEN
  const recoveryPhrase = process.env.SHERPA_BARGE_RECOVERY_PHRASE?.trim() || DEFAULT_RECOVERY_PHRASE

  const { config, label, sttModelPath, ttsModelPath } = resolveVoiceConfig()
  const timeoutMs = Number(process.env.SHERPA_COUNTING_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
  const minNumberWords = Number(
    process.env.SHERPA_COUNTING_MIN_NUMBER_WORDS ?? DEFAULT_MIN_NUMBER_WORDS,
  )
  const minEchoWords = Number(process.env.SHERPA_COUNTING_ECHO_MIN_WORDS ?? DEFAULT_ECHO_MIN_WORDS)
  const minEchoRetention = Number(
    process.env.SHERPA_ECHO_MIN_RETENTION ?? DEFAULT_MIN_ECHO_RETENTION,
  )
  const bargeDelayMs = Number(process.env.SHERPA_BARGE_RECOVERY_DELAY_MS ?? DEFAULT_BARGE_DELAY_MS)
  const bargeToneS = Number(process.env.SHERPA_BARGE_RECOVERY_TONE_S ?? DEFAULT_BARGE_TONE_S)
  const maxInterruptWords = Number(
    process.env.SHERPA_BARGE_RECOVERY_MAX_NUMBER_WORDS ?? DEFAULT_MAX_INTERRUPT_NUMBER_WORDS,
  )
  const maxInterruptSimilarity = Number(
    process.env.SHERPA_BARGE_RECOVERY_MAX_SIMILARITY ?? DEFAULT_MAX_INTERRUPT_SIMILARITY,
  )
  const guardMs = config.vad?.bargeIn?.agentPlaybackGuardMs ?? 1200

  const finalizeWaitMs = sttFinalizeWaitMs(config)
  const postTtsSilenceS = postTtsSilenceSeconds(config)
  const interLegGapS = Number(
    process.env.SHERPA_COUNTING_INTER_LEG_GAP_S ?? DEFAULT_INTER_LEG_GAP_S,
  )
  const interRoundGapS = Number(
    process.env.SHERPA_COUNTING_INTER_ROUND_GAP_S ?? DEFAULT_INTER_ROUND_GAP_S,
  )
  const verbose = process.env.SHERPA_COUNTING_VERBOSE === '1'

  console.log('=== Sherpa counting barge-in recovery roundtrip ===')
  console.log(`Pipeline: ${label}`)
  console.log(
    `VAD: gateStt=${config.vad?.gateStt !== false}  sttGateHold=${config.vad?.sttGateHoldMs ?? VOICE_AGENT_VAD_PRESET.sttGateHoldMs}ms  agentPlaybackGuardMs=${guardMs}`,
  )
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log(`Steps: (1) count→You said OK  (2) count→barge→partial  (3) recovery→You said OK`)
  console.log(
    `Barge: delay=${bargeDelayMs}ms  tone=${bargeToneS}s  maxWords=${maxInterruptWords}  maxSim=${maxInterruptSimilarity}`,
  )
  console.log('')

  const { agentOut, userInbound, userOut, agentInbound, cleanup } =
    await createBidirectionalLoopback()

  const agent1 = new VoiceAgent({
    stt: config.stt,
    tts: config.tts,
    events: { mode: 'stream' },
    vad: agent1VadConfig(config),
  })
  const agent2 = new VoiceAgent({
    stt: config.stt,
    tts: config.tts,
    events: { mode: 'stream' },
    vad: agent2VadConfig(config),
  })

  await agent1.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await agent2.attach({ inboundTrack: userInbound, outboundTrack: userOut })
  await agent1.start()
  await agent2.start()

  const warmupS = Number(process.env.SHERPA_ROUNDTRIP_WARMUP_S ?? DEFAULT_WARMUP_S)
  await Promise.all([streamSilence(agentOut, warmupS), streamSilence(userOut, warmupS)])

  const collectorAgent1 = new ListenerUtteranceCollector(
    agent1,
    { value: false },
    verbose,
    'agent1',
  )
  const collectorAgent2 = new ListenerUtteranceCollector(
    agent2,
    { value: false },
    verbose,
    'agent2',
  )
  collectorAgent1.startPump()
  collectorAgent2.startPump()

  const roundParams = {
    agent1,
    agent2,
    agentOut,
    userOut,
    collectorAgent1,
    collectorAgent2,
    postTtsSilenceS,
    timeoutMs,
    finalizeWaitMs,
    interLegGapS,
    minNumberWords,
    minEchoNumberWords: minEchoWords,
    minSimilarity: 0.75,
    minEchoSimilarity: 0.6,
    minEchoRetention,
  }

  const failures: string[] = []

  const round1 = await runEchoRound({
    ...roundParams,
    name: 'counting 1 (baseline)',
    kind: 'counting',
    sourcePhrase: countingPhrase,
  })
  if (!round1.passed) failures.push(...round1.failures)
  else console.log('✓ Round 1: full You said counting')

  await streamSilence(agentOut, interRoundGapS)
  await streamSilence(userOut, interRoundGapS)

  console.log('')
  console.log('=== Round 2: counting → barge-in during You said ===')

  const legA2Recognized = await playTtsAndCollect({
    speaker: agent1,
    speakerOut: agentOut,
    listenerCollector: collectorAgent2,
    text: countingPhrase,
    postTtsSilenceS,
    timeoutMs,
    finalizeWaitMs,
    logLabel: 'counting 2 agent1→agent2',
  })

  const legA2 = evaluateCountingEchoLeg({
    phrase: countingPhrase,
    recognized: legA2Recognized,
    stats: collectorAgent2.stats,
    label: 'counting 2 Agent2 heard Agent1',
    minNumberWords,
  })
  if (!legA2.passed) failures.push(...legA2.failures)

  await streamSilence(agentOut, interLegGapS)
  await streamSilence(userOut, interLegGapS)

  const echoText2 = formatAgent2EchoReply(legA2.recognized)
  const legB2Recognized = await playEchoLegBWithBargeIn({
    agent1,
    agent2,
    agentOut,
    userOut,
    collectorAgent1,
    collectorAgent2,
    echoText: echoText2,
    bargeDelayMs,
    bargeToneS,
    postTtsSilenceS,
    timeoutMs,
    finalizeWaitMs,
    logLabel: 'counting 2 agent2→agent1 (barge)',
  })

  const interrupted = evaluateInterruptedEchoLeg({
    echoText: echoText2,
    recognized: legB2Recognized,
    maxNumberWords: maxInterruptWords,
    maxSimilarity: maxInterruptSimilarity,
  })

  console.log(`Leg B (interrupted) recognized: "${interrupted.recognized}"`)
  console.log(
    `Leg B partial check: numbers=${interrupted.numberWordsFound}/10  similarity=${(interrupted.similarity * 100).toFixed(0)}%`,
  )

  if (!interrupted.passed) failures.push(...interrupted.failures)
  else console.log('✓ Round 2: barge-in truncated You said playback')

  await streamSilence(agentOut, interRoundGapS)
  await streamSilence(userOut, interRoundGapS)

  const round3 = await runEchoRound({
    ...roundParams,
    name: 'recovery phrase',
    kind: 'sentence',
    sourcePhrase: recoveryPhrase,
  })
  if (!round3.passed) failures.push(...round3.failures)
  else {
    const retention = wordSimilarity(legA2.recognized, round3.legB.recognized)
    console.log(
      `✓ Round 3: recovery You said OK (retention vs count2: ${(retention * 100).toFixed(0)}%)`,
    )
    if (!transcriptIncludesYouSaid(round3.legB.recognized)) {
      failures.push('recovery leg B: missing "you said" in transcript')
    }
  }

  await agent1.stop().catch(() => undefined)
  await agent2.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  if (failures.length > 0) {
    const legs = [
      round1
        ? {
            label: `${round1.name} leg A`,
            phrase: round1.sourcePhrase,
            recognized: round1.legA.recognized,
            stats: round1.legA.stats,
          }
        : undefined,
      round1
        ? {
            label: `${round1.name} leg B`,
            recognized: round1.legB.recognized,
            stats: round1.legB.stats,
          }
        : undefined,
      {
        label: 'counting 2 Agent2 heard Agent1',
        phrase: countingPhrase,
        recognized: legA2Recognized,
        stats: collectorAgent2.stats,
      },
      {
        label: 'counting 2 leg B (barge interrupt)',
        recognized: legB2Recognized,
        stats: collectorAgent1.stats,
      },
      round3
        ? {
            label: `${round3.name} leg A`,
            phrase: round3.sourcePhrase,
            recognized: round3.legA.recognized,
            stats: round3.legA.stats,
          }
        : undefined,
      round3
        ? {
            label: `${round3.name} leg B`,
            recognized: round3.legB.recognized,
            stats: round3.legB.stats,
          }
        : undefined,
    ].filter((leg): leg is NonNullable<typeof leg> => leg !== undefined)

    exitSherpaRoundtripFailure({
      reason: 'barge-in recovery assertions failed',
      failures,
      legs,
    })
  }

  console.log('\nBarge-in recovery roundtrip OK — full echo, truncated barge, recovery echo.')
  process.exit(0)
}

const isMain = process.argv[1]?.endsWith('roundtrip-counting-barge-recovery.ts') === true

if (isMain) {
  main().catch((error: unknown) => {
    exitSherpaRoundtripFailure({
      reason: 'uncaught error',
      error,
    })
  })
}
