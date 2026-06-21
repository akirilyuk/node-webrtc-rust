/**
 * Sherpa roundtrip — three independent speaker→listener legs with concurrent TTS.
 *
 * Each leg is a bidirectional WebRTC loopback (like other roundtrips). All three speakers
 * call `sendTextToTTS(..., { nonBlocking: true })` at once so synthesis can overlap across
 * sessions when `SHERPA_POOL_MAX_CONCURRENT_TTS` allows.
 *
 * Run:
 *   npm run start:roundtrip-concurrent-multi-client --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Env:
 *   SHERPA_CONCURRENT_TIMEOUT_MS     per-leg STT wait (default 90000)
 *   SHERPA_CONCURRENT_MAX_ENQUEUE_MS max wall time for nonBlocking enqueue (default 200)
 *   SHERPA_CONCURRENT_LEGS            leg count 1–6 (default 3)
 *   SHERPA_CONCURRENT_OBSERVE=1       log timing spreads but do not fail on them (6-leg + pool=3)
 *   SHERPA_POOL_MAX_CONCURRENT_TTS   recommend 3 for CI; use 3 with 6 legs to see queueing
 */

import type { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import { VoiceAgent, VOICE_AGENT_VAD_PRESET } from '@node-webrtc-rust/sdk/voice'
import type { VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import {
  evaluateConcurrentRoundtrip,
  type ConcurrentLegResult,
} from './roundtrip-concurrent-timing-helpers.js'
import {
  AgentSpeakingEndLatch,
  ListenerUtteranceCollector,
  installRoundtripWallClockTimeout,
  postTtsSilenceSeconds,
  roundtripWallClockMs,
  sttFinalizeWaitMs,
} from './roundtrip-counting.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import { logRoundtripSpeechEvent } from './roundtrip-speech-events.js'
import { streamSilence } from './pcm-relay.js'
import { resolveVoiceConfig } from './resolve-voice-config.js'

const DEFAULT_TIMEOUT_MS = 90_000
const DEFAULT_MAX_ENQUEUE_MS = 200
const DEFAULT_WARMUP_S = 0.6
const MAX_AGENT_START_SPREAD_MS = 500
const MAX_FINAL_SPREAD_MS = 500

/** Distinct phrases — keyword is a token Sherpa usually preserves (not always NATO). */
const LEG_CATALOG = [
  { phrase: 'alpha one two three', keyword: 'alpha' },
  { phrase: 'bravo four five six', keyword: 'four' },
  { phrase: 'delta seven eight nine', keyword: 'delta' },
  { phrase: 'echo ten eleven twelve', keyword: 'ten' },
  { phrase: 'foxtrot thirteen fourteen fifteen', keyword: 'thirteen' },
  { phrase: 'golf sixteen seventeen eighteen', keyword: 'sixteen' },
] as const

export function resolveConcurrentLegs(
  legCount = Number(process.env.SHERPA_CONCURRENT_LEGS ?? 3),
): Array<{
  legId: string
  phrase: string
  keyword: string
}> {
  const count = Math.max(1, Math.min(LEG_CATALOG.length, legCount))
  return LEG_CATALOG.slice(0, count).map((leg, index) => ({
    legId: `client-tab${index + 1}`,
    phrase: leg.phrase,
    keyword: leg.keyword,
  }))
}

interface LegRuntime {
  legId: string
  phrase: string
  keyword: string
  speaker: VoiceAgent
  listener: VoiceAgent
  agentOut: LocalAudioTrack
  collector: ListenerUtteranceCollector
  speakerEndLatch: AgentSpeakingEndLatch
  cleanup: () => Promise<void>
  agentSpeakingStartMs: number | null
  finalText: string | null
  finalMs: number | null
}

function startSpeakerTimingPump(leg: LegRuntime, verbose: boolean): void {
  void (async () => {
    for await (const event of leg.speaker.speechEvents()) {
      if (verbose) logRoundtripSpeechEvent(`speaker-${leg.legId}`, event)
      leg.speakerEndLatch.observe(event)
      if (event.type === 'agent_speaking_start' && leg.agentSpeakingStartMs == null) {
        leg.agentSpeakingStartMs = performance.now()
      }
    }
  })()
}

async function setupLeg(
  config: VoiceAgentConfig,
  legId: string,
  phrase: string,
  keyword: string,
  verbose: boolean,
): Promise<LegRuntime> {
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

  const pumpStarted = { value: false }
  const collector = new ListenerUtteranceCollector(listener, pumpStarted, verbose, legId)
  collector.startPump()

  const runtime: LegRuntime = {
    legId,
    phrase,
    keyword,
    speaker,
    listener,
    agentOut,
    collector,
    speakerEndLatch: new AgentSpeakingEndLatch(),
    cleanup,
    agentSpeakingStartMs: null,
    finalText: null,
    finalMs: null,
  }

  startSpeakerTimingPump(runtime, verbose)

  await streamSilence(agentOut, DEFAULT_WARMUP_S)
  return runtime
}

async function main(): Promise<void> {
  if (!process.env.SHERPA_POOL_MAX_CONCURRENT_TTS) {
    process.env.SHERPA_POOL_MAX_CONCURRENT_TTS = '3'
  }
  const { config, label, sttModelPath, ttsModelPath } = resolveVoiceConfig()
  const legDefs = resolveConcurrentLegs()
  const wallMs =
    Number(process.env.SHERPA_ROUNDTRIP_WALL_MS) ||
    roundtripWallClockMs(config, legDefs.length > 3 ? 'long' : 'long') +
      (legDefs.length > 3 ? 60_000 : 0)
  installRoundtripWallClockTimeout(wallMs)
  const timeoutMs = Number(process.env.SHERPA_CONCURRENT_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
  const maxEnqueueMs = Number(
    process.env.SHERPA_CONCURRENT_MAX_ENQUEUE_MS ?? DEFAULT_MAX_ENQUEUE_MS,
  )
  const finalizeWaitMs = sttFinalizeWaitMs(config)
  const postTtsSilenceS = postTtsSilenceSeconds(config)
  const verbose = process.env.SHERPA_COUNTING_VERBOSE === '1'
  const ttsPool = Number(process.env.SHERPA_POOL_MAX_CONCURRENT_TTS ?? 2)
  const observeOnly = process.env.SHERPA_CONCURRENT_OBSERVE === '1' || legDefs.length > ttsPool
  const maxAgentStartSpreadMs = Number(
    process.env.SHERPA_CONCURRENT_MAX_AGENT_START_SPREAD_MS ??
      (observeOnly ? 60_000 : MAX_AGENT_START_SPREAD_MS),
  )
  const maxFinalSpreadMs = Number(
    process.env.SHERPA_CONCURRENT_MAX_FINAL_SPREAD_MS ??
      (observeOnly ? 120_000 : MAX_FINAL_SPREAD_MS),
  )

  console.log(`=== Sherpa concurrent multi-client roundtrip (${legDefs.length} legs) ===`)
  console.log(`Pipeline: ${label}`)
  console.log(
    `Listener: gateStt=${config.vad?.gateStt !== false}  minSilence=${config.vad?.minSilenceDurationMs ?? VOICE_AGENT_VAD_PRESET.minSilenceDurationMs}ms`,
  )
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log(
    `SHERPA_POOL_MAX_CONCURRENT_TTS=${process.env.SHERPA_POOL_MAX_CONCURRENT_TTS ?? '(default)'}`,
  )
  if (observeOnly) {
    console.log(
      `Observe mode: timing spreads logged (max agent_start=${maxAgentStartSpreadMs}ms final=${maxFinalSpreadMs}ms); enqueue + STT keywords still required`,
    )
  }
  console.log('')

  const legs = await Promise.all(
    legDefs.map((leg) => setupLeg(config, leg.legId, leg.phrase, leg.keyword, verbose)),
  )

  const finalPromises = legs.map((leg) =>
    leg.collector.waitForNext(timeoutMs, finalizeWaitMs).then((text) => {
      leg.finalText = text
      leg.finalMs = performance.now()
      return text
    }),
  )

  const endBaselines = legs.map((leg) => leg.speakerEndLatch.endEventsSeen())

  const t0 = performance.now()
  await Promise.all(legs.map((leg) => leg.speaker.sendTextToTTS(leg.phrase, { nonBlocking: true })))
  const enqueueMs = performance.now() - t0
  console.log(`[concurrent] nonBlocking enqueue for ${legs.length} legs: ${enqueueMs.toFixed(0)}ms`)

  for (let i = 0; i < legs.length; i++) {
    const leg = legs[i]!
    const baseline = endBaselines[i]!
    void (async () => {
      try {
        await leg.speakerEndLatch.waitAfterCount(baseline, 45_000)
      } catch {
        /* proceed with trailing silence anyway */
      }
      await streamSilence(leg.agentOut, postTtsSilenceS)
    })()
  }

  const finals = await Promise.all(finalPromises)

  for (let i = 0; i < legs.length; i++) {
    console.log(
      `[${legs[i]!.legId}] agent_speaking_start=${legs[i]!.agentSpeakingStartMs?.toFixed(0) ?? 'missing'} final="${finals[i]!.slice(0, 80)}"`,
    )
  }

  const evaluation = evaluateConcurrentRoundtrip({
    legs: legs.map(
      (leg): ConcurrentLegResult => ({
        legId: leg.legId,
        phrase: leg.phrase,
        keyword: leg.keyword,
        agentSpeakingStartMs: leg.agentSpeakingStartMs,
        finalText: leg.finalText,
        finalMs: leg.finalMs,
      }),
    ),
    enqueueMs,
    maxEnqueueMs,
    maxAgentStartSpreadMs,
    maxFinalSpreadMs,
  })

  const keywordFailures = evaluation.failures.filter(
    (f) => f.includes('missing user_speech_final') || f.includes('missing keyword'),
  )
  const timingFailures = evaluation.failures.filter((f) => !keywordFailures.includes(f))

  if (observeOnly && timingFailures.length > 0) {
    console.log('')
    console.log('=== Timing (observe — not failing) ===')
    for (const failure of timingFailures) {
      console.log(`  - ${failure}`)
    }
  }

  for (const leg of legs) {
    await leg.listener.stop().catch(() => undefined)
    await leg.speaker.stop().catch(() => undefined)
    await leg.cleanup().catch(() => undefined)
  }

  if (!evaluation.passed) {
    const hardFailures = observeOnly ? keywordFailures : evaluation.failures
    if (hardFailures.length === 0 && observeOnly) {
      console.log('')
      console.log('=== PASS (observe) ===')
      console.log(
        `enqueue=${evaluation.enqueueMs.toFixed(0)}ms agent_start_spread=${evaluation.agentStartWindow.spreadMs.toFixed(0)}ms final_spread=${evaluation.finalWindow.spreadMs.toFixed(0)}ms`,
      )
      process.exit(0)
    }
    exitSherpaRoundtripFailure({
      reason: 'concurrent multi-client roundtrip failed',
      failures: hardFailures,
      legs: legs.map((leg) => ({
        label: leg.legId,
        phrase: leg.phrase,
        recognized: leg.finalText ?? '',
      })),
    })
  }

  console.log('')
  console.log('=== PASS ===')
  console.log(
    `enqueue=${evaluation.enqueueMs.toFixed(0)}ms agent_start_spread=${evaluation.agentStartWindow.spreadMs.toFixed(0)}ms final_spread=${evaluation.finalWindow.spreadMs.toFixed(0)}ms`,
  )
  process.exit(0)
}

main().catch((error: unknown) => {
  exitSherpaRoundtripFailure({
    reason: error instanceof Error ? error.message : String(error),
    failures: [],
    legs: [],
  })
})
