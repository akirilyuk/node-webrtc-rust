/**
 * Bidirectional Sherpa echo roundtrip — multiple rounds on one loopback.
 *
 *   Agent 1 (agent PC)  ──TTS──► agentOut ──► userInbound ──► Agent 2 STT
 *   Agent 2 (user PC)   ──TTS──► userOut  ──► agentInbound ──► Agent 1 STT
 *
 * Each round:
 *   A — Agent 1 speaks the source phrase; Agent 2 must hear **one** final.
 *   B — Agent 2 TTS: `You said: {recognized from A}`; Agent 1 must hear **one** final
 *       that includes "you said" and preserves the content.
 *
 * Default rounds:
 *   1. Count *one* … *ten*
 *   2. Long sentence (stress-test length + VAD)
 *
 * Run:
 *   npm run start:roundtrip-counting-echo --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
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
  DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
  DEFAULT_COUNTING_PHRASE_ONE_TO_TEN,
  evaluateCountingRoundtrip,
  installRoundtripWallClockTimeout,
  ListenerUtteranceCollector,
  normalizeForCompare,
  NUMBER_WORDS_ONE_TO_TEN,
  AgentSpeakingEndLatch,
  playSpeakerTtsWithPostSilence,
  postTtsSilenceSeconds,
  sttFinalizeWaitMs,
  wordSimilarity,
  type UtteranceEventStats,
} from './roundtrip-counting.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'

/** Agent 2 always prefixes echoed speech (matches multi-client voice-handler). */
export const ECHO_REPLY_PREFIX = 'You said: '

/** Peer TTS on the loopback is not agent playback — disable barge so STT can finalize. */
function echoVadConfig(base: VoiceAgentConfig): NonNullable<VoiceAgentConfig['vad']> {
  return {
    ...VOICE_AGENT_VAD_PRESET,
    ...base.vad,
    bargeIn: {
      ...VOICE_AGENT_VAD_PRESET.bargeIn,
      ...base.vad?.bargeIn,
      enabled: false,
    },
  }
}

export const DEFAULT_LONG_SENTENCE_PHRASE =
  'This is a very long sentence, maybe the longest sentence ever spoken. It is so long, that the lenght of it cannot even be measured.'

const DEFAULT_TIMEOUT_MS = 45_000
const DEFAULT_MIN_NUMBER_WORDS_ONE_TO_TEN = 8
const DEFAULT_ECHO_MIN_WORDS = 8
const DEFAULT_MIN_SIMILARITY = 0.75
const DEFAULT_ECHO_MIN_SIMILARITY = 0.6
const DEFAULT_MIN_ECHO_RETENTION = 0.6
const DEFAULT_WARMUP_S = 0.6
const DEFAULT_INTER_LEG_GAP_S = 0.5
const DEFAULT_INTER_ROUND_GAP_S = 1.0

export type EchoRoundKind = 'counting' | 'sentence'

export interface EchoLegResult {
  spokenText: string
  recognized: string
  stats: UtteranceEventStats
  passed: boolean
  failures: string[]
  /** counting: number words hit; sentence: word similarity vs spoken phrase */
  score: number
}

export interface EchoRoundResult {
  name: string
  kind: EchoRoundKind
  sourcePhrase: string
  legA: EchoLegResult
  legB: EchoLegResult
  passed: boolean
  failures: string[]
}

export function formatAgent2EchoReply(recognized: string): string {
  const trimmed = recognized.trim()
  if (!trimmed) return ECHO_REPLY_PREFIX.trim()
  return `${ECHO_REPLY_PREFIX}${trimmed}`
}

/** Echo payload only — omit fixed "You said:" prefix for truncation similarity. */
export function echoPayloadForCompare(text: string): string {
  const norm = normalizeForCompare(text)
  const marker = 'you said'
  const idx = norm.indexOf(marker)
  if (idx === -1) return norm
  return norm.slice(idx + marker.length).replace(/^[\s:]+/, '').trim()
}

export function transcriptIncludesYouSaid(recognized: string): boolean {
  const norm = normalizeForCompare(recognized)
  if (norm.includes('you said') || (norm.includes('you') && norm.includes('said'))) {
    return true
  }
  // Sherpa often mis-hears Piper "You said:" on long echo TTS (e.g. "He uses …").
  const aliases = ['he said', 'he uses', 'you use', 'you says', 'he say']
  return aliases.some((prefix) => norm.includes(prefix))
}

/** Share of leg-A number words that also appear in leg-B transcript. */
export function echoNumberWordRetention(
  legARecognized: string,
  legBRecognized: string,
  numberWords: readonly string[] = NUMBER_WORDS_ONE_TO_TEN,
): number {
  const haystackA = ` ${normalizeForCompare(legARecognized)} `
  const foundInA = numberWords.filter((word) => haystackA.includes(` ${word} `))
  if (foundInA.length === 0) return 0
  const haystackB = ` ${normalizeForCompare(legBRecognized)} `
  const retained = foundInA.filter((word) => haystackB.includes(` ${word} `)).length
  return retained / foundInA.length
}

function evaluateLegEvents(
  stats: UtteranceEventStats,
  recognized: string,
  label: string,
): string[] {
  const failures: string[] = []
  const who = `${label}: `
  if (stats.finals.length !== 1) {
    failures.push(
      `${who}expected exactly 1 user_speech_final, got ${stats.finals.length}: ${stats.finals.map((t) => JSON.stringify(t)).join(', ')}`,
    )
  }
  // STT utterance close may emit speaking_end in the same poll as the final; allow 0 when exactly one final.
  if (
    stats.speakingEndCount !== 1 &&
    !(stats.finals.length === 1 && stats.speakingEndCount === 0)
  ) {
    failures.push(`${who}expected exactly 1 user_speaking_end, got ${stats.speakingEndCount}`)
  }
  if (stats.speakingStartCount < 1) {
    failures.push(`${who}expected at least 1 user_speaking_start, got ${stats.speakingStartCount}`)
  }
  if (!recognized.trim()) {
    failures.push(`${who}recognized transcript is empty`)
  }
  return failures
}

/** Gate-hold may emit a short prefix final then the full echo — evaluate as one utterance when transcript is complete. */
function statsForEchoLegEvaluation(
  stats: UtteranceEventStats,
  recognized: string,
): UtteranceEventStats {
  const text = recognized.trim()
  if (stats.finals.length <= 1 || !text) {
    return stats
  }
  const best = stats.finals.reduce((a, b) => (a.trim().length >= b.trim().length ? a : b))
  if (best.trim().length >= text.length) {
    return {
      ...stats,
      finals: [best.trim()],
      speakingEndCount: 1,
      speakingStartCount: Math.min(1, stats.speakingStartCount),
    }
  }
  return stats
}

export function evaluateCountingEchoLeg(params: {
  phrase: string
  recognized: string
  stats: UtteranceEventStats
  label: string
  minNumberWords: number
  requireYouSaid?: boolean
  /** When set, ≥90% number-word retention from leg A waives a missing "you said" prefix. */
  legASourceForRetention?: string
}): EchoLegResult {
  const stats = statsForEchoLegEvaluation(params.stats, params.recognized)
  const evaluation = evaluateCountingRoundtrip({
    phrase: params.phrase,
    recognized: params.recognized,
    stats,
    minNumberWords: params.minNumberWords,
    numberWords: NUMBER_WORDS_ONE_TO_TEN,
    label: params.label,
  })
  const failures = [...evaluation.failures]
  if (params.requireYouSaid && !transcriptIncludesYouSaid(params.recognized)) {
    const retentionOk =
      params.legASourceForRetention != null &&
      echoNumberWordRetention(params.legASourceForRetention, params.recognized) >= 0.9
    if (!retentionOk) {
      failures.push(`${params.label}: expected "you said" in echo leg transcript`)
    }
  }
  return {
    spokenText: params.phrase,
    recognized: evaluation.recognized,
    stats,
    passed: failures.length === 0,
    failures,
    score: evaluation.numberWordsFound,
  }
}

export function evaluateSentenceEchoLeg(params: {
  phrase: string
  recognized: string
  stats: UtteranceEventStats
  label: string
  minSimilarity: number
  requireYouSaid?: boolean
}): EchoLegResult {
  const recognized = params.recognized.trim()
  const stats = statsForEchoLegEvaluation(params.stats, recognized)
  const failures = evaluateLegEvents(stats, recognized, params.label)
  const similarity = wordSimilarity(params.phrase, recognized)
  if (similarity < params.minSimilarity) {
    failures.push(
      `${params.label}: word similarity ${(similarity * 100).toFixed(0)}% < ${(params.minSimilarity * 100).toFixed(0)}%`,
    )
  }
  if (params.requireYouSaid && !transcriptIncludesYouSaid(recognized)) {
    failures.push(`${params.label}: expected "you said" in echo leg transcript`)
  }
  return {
    spokenText: params.phrase,
    recognized,
    stats,
    passed: failures.length === 0,
    failures,
    score: similarity,
  }
}

export function evaluateEchoRound(params: {
  name: string
  kind: EchoRoundKind
  sourcePhrase: string
  legA: EchoLegResult
  legB: EchoLegResult
  minEchoRetention?: number
}): EchoRoundResult {
  const failures: string[] = []
  if (!params.legA.passed) failures.push(...params.legA.failures)
  if (!params.legB.passed) failures.push(...params.legB.failures)

  if (params.kind === 'counting') {
    const retention = echoNumberWordRetention(params.legA.recognized, params.legB.recognized)
    const minRetention = params.minEchoRetention ?? DEFAULT_MIN_ECHO_RETENTION
    if (retention < minRetention) {
      failures.push(
        `${params.name}: echo number retention ${(retention * 100).toFixed(0)}% < ${(minRetention * 100).toFixed(0)}%`,
      )
    }
  } else {
    const contentSim = wordSimilarity(params.legA.recognized, params.legB.recognized)
    const minContent = params.minEchoRetention ?? DEFAULT_MIN_ECHO_RETENTION
    if (contentSim < minContent) {
      failures.push(
        `${params.name}: echo content similarity ${(contentSim * 100).toFixed(0)}% < ${(minContent * 100).toFixed(0)}%`,
      )
    }
  }

  return {
    name: params.name,
    kind: params.kind,
    sourcePhrase: params.sourcePhrase,
    legA: params.legA,
    legB: params.legB,
    passed: failures.length === 0,
    failures,
  }
}

export async function playTtsAndCollect(params: {
  speaker: VoiceAgent
  speakerOut: LocalAudioTrack
  listenerCollector: ListenerUtteranceCollector
  agentSpeakingEndLatch: AgentSpeakingEndLatch
  text: string
  postTtsSilenceS: number
  timeoutMs: number
  finalizeWaitMs: number
  logLabel: string
}): Promise<string> {
  const preview = params.text.length > 100 ? `${params.text.slice(0, 100)}…` : params.text
  console.log(`[${params.logLabel}] TTS: "${preview}"`)
  const recognizedPromise = params.listenerCollector.waitForNext(
    params.timeoutMs,
    params.finalizeWaitMs,
  )
  await playSpeakerTtsWithPostSilence({
    speaker: params.speaker,
    speakerOut: params.speakerOut,
    phrase: params.text,
    postTtsSilenceS: params.postTtsSilenceS,
    playbackTimeoutMs: DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
    agentSpeakingEndLatch: params.agentSpeakingEndLatch,
  })
  const recognized = await recognizedPromise
  // Long echo TTS can trigger an early prefix final ("You said") then the full phrase — use the best transcript.
  if (params.listenerCollector.stats.finals.length > 1) {
    const best = params.listenerCollector.stats.finals.reduce((a, b) =>
      a.trim().length >= b.trim().length ? a : b,
    )
    if (best.trim().length > recognized.trim().length) {
      return best.trim()
    }
  }
  return recognized
}

export async function runEchoRound(params: {
  name: string
  kind: EchoRoundKind
  sourcePhrase: string
  agent1: VoiceAgent
  agent2: VoiceAgent
  agentOut: LocalAudioTrack
  userOut: LocalAudioTrack
  collectorAgent1: ListenerUtteranceCollector
  collectorAgent2: ListenerUtteranceCollector
  agent1EndLatch: AgentSpeakingEndLatch
  agent2EndLatch: AgentSpeakingEndLatch
  postTtsSilenceS: number
  timeoutMs: number
  finalizeWaitMs: number
  interLegGapS: number
  minNumberWords: number
  minEchoNumberWords: number
  minSimilarity: number
  minEchoSimilarity: number
  minEchoRetention: number
}): Promise<EchoRoundResult> {
  console.log('')
  console.log(`=== Round: ${params.name} (${params.kind}) ===`)
  console.log(
    `Agent1 speaks: "${params.sourcePhrase.slice(0, 90)}${params.sourcePhrase.length > 90 ? '…' : ''}"`,
  )

  const legARecognized = await playTtsAndCollect({
    speaker: params.agent1,
    speakerOut: params.agentOut,
    listenerCollector: params.collectorAgent2,
    agentSpeakingEndLatch: params.agent1EndLatch,
    text: params.sourcePhrase,
    postTtsSilenceS: params.postTtsSilenceS,
    timeoutMs: params.timeoutMs,
    finalizeWaitMs: params.finalizeWaitMs,
    logLabel: `${params.name} agent1→agent2`,
  })

  const legA =
    params.kind === 'counting'
      ? evaluateCountingEchoLeg({
          phrase: params.sourcePhrase,
          recognized: legARecognized,
          stats: params.collectorAgent2.stats,
          label: `${params.name} Agent2 heard Agent1`,
          minNumberWords: params.minNumberWords,
        })
      : evaluateSentenceEchoLeg({
          phrase: params.sourcePhrase,
          recognized: legARecognized,
          stats: params.collectorAgent2.stats,
          label: `${params.name} Agent2 heard Agent1`,
          minSimilarity: params.minSimilarity,
        })

  console.log(`Leg A recognized: "${legA.recognized}"`)
  console.log(
    `Leg A events: finals=${legA.stats.finals.length} speaking_end=${legA.stats.speakingEndCount} score=${legA.score}`,
  )

  if (!legA.passed) {
    return evaluateEchoRound({
      name: params.name,
      kind: params.kind,
      sourcePhrase: params.sourcePhrase,
      legA,
      legB: {
        spokenText: '',
        recognized: '',
        stats: {
          finals: [],
          speakingEndCount: 0,
          speakingStartCount: 0,
          partialCount: 0,
          bargeInCount: 0,
          agentSpeakingStartCount: 0,
          agentSpeakingEndCount: 0,
          speakingEndAtMs: null,
          speechFinalAtMs: null,
        },
        passed: false,
        failures: [`${params.name}: skipped leg B — leg A failed`],
        score: 0,
      },
    })
  }

  await streamSilence(params.agentOut, params.interLegGapS)
  await streamSilence(params.userOut, params.interLegGapS)

  const echoText = formatAgent2EchoReply(legA.recognized)
  const legBRecognized = await playTtsAndCollect({
    speaker: params.agent2,
    speakerOut: params.userOut,
    listenerCollector: params.collectorAgent1,
    agentSpeakingEndLatch: params.agent2EndLatch,
    text: echoText,
    postTtsSilenceS: params.postTtsSilenceS,
    timeoutMs: params.timeoutMs,
    finalizeWaitMs: params.finalizeWaitMs,
    logLabel: `${params.name} agent2→agent1 (You said: …)`,
  })

  const legB =
    params.kind === 'counting'
      ? evaluateCountingEchoLeg({
          phrase: echoText,
          recognized: legBRecognized,
          stats: params.collectorAgent1.stats,
          label: `${params.name} Agent1 heard Agent2`,
          minNumberWords: params.minEchoNumberWords,
          requireYouSaid: true,
          legASourceForRetention: legA.recognized,
        })
      : evaluateSentenceEchoLeg({
          phrase: echoText,
          recognized: legBRecognized,
          stats: params.collectorAgent1.stats,
          label: `${params.name} Agent1 heard Agent2`,
          minSimilarity: params.minEchoSimilarity,
          requireYouSaid: true,
        })

  console.log(`Leg B TTS: "${echoText.slice(0, 90)}${echoText.length > 90 ? '…' : ''}"`)
  console.log(`Leg B recognized: "${legB.recognized}"`)
  console.log(
    `Leg B events: finals=${legB.stats.finals.length} speaking_end=${legB.stats.speakingEndCount} score=${(legB.score * 100).toFixed(0)}%`,
  )

  return evaluateEchoRound({
    name: params.name,
    kind: params.kind,
    sourcePhrase: params.sourcePhrase,
    legA,
    legB,
    minEchoRetention: params.minEchoRetention,
  })
}

async function main(): Promise<void> {
  installRoundtripWallClockTimeout(180_000)

  const countingPhrase =
    process.env.SHERPA_COUNTING_PHRASE?.trim() || DEFAULT_COUNTING_PHRASE_ONE_TO_TEN
  const longSentencePhrase =
    process.env.SHERPA_ECHO_LONG_SENTENCE?.trim() || DEFAULT_LONG_SENTENCE_PHRASE

  const { config, label, sttModelPath, ttsModelPath } = resolveVoiceConfig()
  const timeoutMs = Number(process.env.SHERPA_COUNTING_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
  const minNumberWords = Number(
    process.env.SHERPA_COUNTING_MIN_NUMBER_WORDS ?? DEFAULT_MIN_NUMBER_WORDS_ONE_TO_TEN,
  )
  const minEchoWords = Number(process.env.SHERPA_COUNTING_ECHO_MIN_WORDS ?? DEFAULT_ECHO_MIN_WORDS)
  const minSimilarity = Number(process.env.SHERPA_ECHO_MIN_SIMILARITY ?? DEFAULT_MIN_SIMILARITY)
  const minEchoSimilarity = Number(
    process.env.SHERPA_ECHO_LEG_MIN_SIMILARITY ?? DEFAULT_ECHO_MIN_SIMILARITY,
  )
  const minEchoRetention = Number(
    process.env.SHERPA_ECHO_MIN_RETENTION ?? DEFAULT_MIN_ECHO_RETENTION,
  )
  const finalizeWaitMs = sttFinalizeWaitMs(config)
  const postTtsSilenceS = postTtsSilenceSeconds(config)
  const interLegGapS = Number(
    process.env.SHERPA_COUNTING_INTER_LEG_GAP_S ?? DEFAULT_INTER_LEG_GAP_S,
  )
  const interRoundGapS = Number(
    process.env.SHERPA_COUNTING_INTER_ROUND_GAP_S ?? DEFAULT_INTER_ROUND_GAP_S,
  )
  const verbose = process.env.SHERPA_COUNTING_VERBOSE === '1'

  console.log('=== Sherpa echo roundtrip (Agent1 ↔ Agent2, multi-round) ===')
  console.log(`Pipeline: ${label}`)
  console.log(
    `VAD: gateStt=${config.vad?.gateStt !== false}  minSilence=${config.vad?.minSilenceDurationMs ?? VOICE_AGENT_VAD_PRESET.minSilenceDurationMs}ms  sttGateHold=${config.vad?.sttGateHoldMs ?? VOICE_AGENT_VAD_PRESET.sttGateHoldMs}ms`,
  )
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log(`Agent2 echo prefix: "${ECHO_REPLY_PREFIX}"`)
  console.log(`Round 1 (counting): "${countingPhrase}"`)
  console.log(`Round 2 (long sentence): "${longSentencePhrase.slice(0, 70)}…"`)
  console.log(`Timing: postTtsSilence=${postTtsSilenceS.toFixed(1)}s  timeout=${timeoutMs}ms`)
  console.log('')

  const { agentOut, userInbound, userOut, agentInbound, cleanup } =
    await createBidirectionalLoopback()

  const echoVad = echoVadConfig(config)
  const agent1 = new VoiceAgent({
    stt: config.stt,
    tts: config.tts,
    events: { mode: 'stream' },
    vad: echoVad,
  })
  const agent2 = new VoiceAgent({
    stt: config.stt,
    tts: config.tts,
    events: { mode: 'stream' },
    vad: echoVad,
  })

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
  const collectorAgent2 = new ListenerUtteranceCollector(
    agent2,
    { value: false },
    verbose,
    'agent2',
    agent2EndLatch,
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
    agent1EndLatch,
    agent2EndLatch,
    postTtsSilenceS,
    timeoutMs,
    finalizeWaitMs,
    interLegGapS,
    minNumberWords,
    minEchoNumberWords: minEchoWords,
    minSimilarity,
    minEchoSimilarity,
    minEchoRetention,
  }

  const rounds: EchoRoundResult[] = []

  rounds.push(
    await runEchoRound({
      ...roundParams,
      name: 'counting one–ten',
      kind: 'counting',
      sourcePhrase: countingPhrase,
    }),
  )

  if (!rounds[0]!.passed) {
    await agent1.stop().catch(() => undefined)
    await agent2.stop().catch(() => undefined)
    await cleanup().catch(() => undefined)
    const r = rounds[0]!
    exitSherpaRoundtripFailure({
      reason: `round 1 failed (${r.name})`,
      failures: r.failures,
      legs: [
        {
          label: `${r.name} leg A`,
          phrase: r.sourcePhrase,
          recognized: r.legA.recognized,
          stats: r.legA.stats,
        },
        { label: `${r.name} leg B (echo)`, recognized: r.legB.recognized, stats: r.legB.stats },
      ],
    })
  }

  await streamSilence(agentOut, interRoundGapS)
  await streamSilence(userOut, interRoundGapS)

  rounds.push(
    await runEchoRound({
      ...roundParams,
      name: 'long sentence',
      kind: 'sentence',
      sourcePhrase: longSentencePhrase,
    }),
  )

  await agent1.stop().catch(() => undefined)
  await agent2.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  const allFailures = rounds.flatMap((r) => r.failures)
  if (allFailures.length > 0) {
    const failedRound = rounds.find((r) => !r.passed) ?? rounds[rounds.length - 1]!
    exitSherpaRoundtripFailure({
      reason: failedRound.passed ? 'assertions failed' : `round failed (${failedRound.name})`,
      failures: allFailures,
      legs: [
        {
          label: `${failedRound.name} leg A`,
          phrase: failedRound.sourcePhrase,
          recognized: failedRound.legA.recognized,
          stats: failedRound.legA.stats,
        },
        {
          label: `${failedRound.name} leg B (echo)`,
          recognized: failedRound.legB.recognized,
          stats: failedRound.legB.stats,
        },
      ],
    })
  }

  console.log(
    '\nEcho roundtrip OK — all rounds passed (1× final per leg, Agent2 uses "You said: …").',
  )
  for (const r of rounds) {
    console.log(`  ✓ ${r.name}`)
  }
  process.exit(0)
}

const isMain = process.argv[1]?.endsWith('roundtrip-counting-echo.ts') === true

if (isMain) {
  main().catch((error: unknown) => {
    exitSherpaRoundtripFailure({
      reason: 'uncaught error',
      error,
    })
  })
}
