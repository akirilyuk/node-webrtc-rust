/**
 * Sherpa load roundtrip — N concurrent speaker→listener legs, several sequential turns each.
 *
 * Each leg is a bidirectional WebRTC loopback (like the other roundtrips) with a Sherpa TTS
 * speaker and a Sherpa STT listener. All legs run in one process at the same time, so the run
 * exercises the full stack (TTS pool, STT decode, VAD, WebRTC) under concurrent load.
 *
 * Phrase cache: the npm scripts `start:roundtrip-load` and `start:roundtrip-load-ci` set
 * `SHERPA_TTS_PHRASE_CACHE=0`, so every turn runs a real synthesis (six phrases repeat, so with the
 * cache on, TTS would be served from memory and synthesis load would not be measured).
 * `start:roundtrip-load-cached` keeps the cache on.
 *
 * Capacity mode judges the 2-leg step only functionally; its final-latency p95 becomes the baseline.
 * Later steps pass while they have no failures and p95 <= baseline + SHERPA_LOAD_SLO_DELTA_MS
 * (final latency has a fixed VAD/gate-hold floor, so an absolute SLO could not detect saturation).
 *
 * Gated (non-zero exit): every turn must produce a `user_speech_final` that contains the
 * leg's keyword, and nothing may throw or time out.
 * Reported only (never gated): latency percentiles, CPU per session-minute, event-loop lag,
 * RSS. One `perf_probe <json>` line per run (per step in capacity mode).
 *
 * Measurements cover the turn phase only (after all legs are attached and warmed up):
 * `process.cpuUsage()` delta, `monitorEventLoopDelay` p99, wall time; RSS is sampled at the end.
 *
 * Run:
 *   npm run start:roundtrip-load --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *   SHERPA_LOAD_CAPACITY=1 SHERPA_LOAD_TURNS=2 npm run start:roundtrip-load --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Env:
 *   SHERPA_LOAD_LEGS             concurrent legs in normal mode (default 8)
 *   SHERPA_LOAD_TURNS            sequential turns per leg (default 5)
 *   SHERPA_LOAD_CAPACITY         1 = capacity mode: 2, 4, 6, 8, 10, 12, 16, 20, 24, 32, 40, 48 legs until a step fails
 *   SHERPA_LOAD_CAPACITY_MAX_LEGS capacity mode: last step to try (default 48)
 *   SHERPA_LOAD_SLO_DELTA_MS     capacity mode: a step passes while its final-latency p95 is at most
 *                                the 2-leg step's p95 plus this (default 1000)
 *   SHERPA_LOAD_TURN_TIMEOUT_MS  per-turn STT wait (default 90000)
 *   SHERPA_ROUNDTRIP_WALL_MS     overall wall-clock limit (default derived from legs/turns)
 *   SHERPA_COUNTING_VERBOSE=1    log speech events per leg
 */

import { monitorEventLoopDelay } from 'node:perf_hooks'

import type { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import { VoiceAgent, VOICE_AGENT_VAD_PRESET } from '@node-webrtc-rust/sdk/voice'
import type { VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { LEG_CATALOG } from './roundtrip-concurrent-timing-helpers.js'
import {
  AgentSpeakingEndLatch,
  ListenerUtteranceCollector,
  installRoundtripWallClockTimeout,
  postTtsSilenceSeconds,
  sttFinalizeWaitMs,
} from './roundtrip-counting.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import {
  CAPACITY_STEPS,
  capacityStepPasses,
  evaluateLoadRun,
  nextCapacityStep,
  type LoadRunEvaluation,
  type LoadTurnResult,
} from './roundtrip-load-helpers.js'
import { logRoundtripSpeechEvent } from './roundtrip-speech-events.js'
import { streamSilence } from './pcm-relay.js'
import { resolveRoundtripVoiceConfig } from './resolve-voice-config.js'

const DEFAULT_LEGS = 8
const DEFAULT_TURNS = 5
const DEFAULT_SLO_DELTA_MS = 1000
const DEFAULT_CAPACITY_MAX_LEGS = 48
const DEFAULT_TURN_TIMEOUT_MS = 90_000
const DEFAULT_WARMUP_S = 0.6
const SPEAKER_END_WAIT_MS = 45_000

interface LoadLeg {
  index: number
  legId: string
  speaker: VoiceAgent
  listener: VoiceAgent
  agentOut: LocalAudioTrack
  collector: ListenerUtteranceCollector
  speakerEndLatch: AgentSpeakingEndLatch
  cleanup: () => Promise<void>
  agentStartTimesMs: number[]
  agentEndTimesMs: number[]
}

function envPositiveInt(name: string, fallback: number): number {
  const raw = Number(process.env[name])
  return Number.isFinite(raw) && raw > 0 ? Math.floor(raw) : fallback
}

function sleepMs(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function startSpeakerPump(leg: LoadLeg, verbose: boolean): void {
  void (async () => {
    for await (const event of leg.speaker.speechEvents()) {
      if (verbose) logRoundtripSpeechEvent(`speaker-${leg.legId}`, event)
      if (event.type === 'agent_speaking_start') leg.agentStartTimesMs.push(performance.now())
      // Record the end time before the latch resolves its waiters.
      if (event.type === 'agent_speaking_end') leg.agentEndTimesMs.push(performance.now())
      leg.speakerEndLatch.observe(event)
    }
  })()
}

async function setupLeg(
  config: VoiceAgentConfig,
  index: number,
  verbose: boolean,
): Promise<LoadLeg> {
  const legId = `load-leg${index + 1}`
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

  const leg: LoadLeg = {
    index,
    legId,
    speaker,
    listener,
    agentOut,
    collector,
    speakerEndLatch: new AgentSpeakingEndLatch(),
    cleanup,
    agentStartTimesMs: [],
    agentEndTimesMs: [],
  }
  startSpeakerPump(leg, verbose)

  await streamSilence(agentOut, DEFAULT_WARMUP_S)
  return leg
}

async function teardownLegs(legs: LoadLeg[]): Promise<void> {
  await Promise.all(
    legs.map(async (leg) => {
      await leg.listener.stop().catch(() => undefined)
      await leg.speaker.stop().catch(() => undefined)
      await leg.cleanup().catch(() => undefined)
    }),
  )
}

async function runTurn(
  leg: LoadLeg,
  turnIndex: number,
  timing: { timeoutMs: number; finalizeWaitMs: number; postTtsSilenceS: number },
): Promise<LoadTurnResult> {
  const { phrase, keyword } = LEG_CATALOG[(leg.index + turnIndex) % LEG_CATALOG.length]!
  const result: LoadTurnResult = {
    legId: leg.legId,
    turn: turnIndex + 1,
    keyword,
    finalText: null,
    finalLatencyMs: null,
    ttsStartLatencyMs: null,
  }
  try {
    const startsBaseline = leg.agentStartTimesMs.length
    const endsBaseline = leg.speakerEndLatch.endEventsSeen()
    let finalAtMs: number | null = null
    const finalPromise = leg.collector
      .waitForNext(timing.timeoutMs, timing.finalizeWaitMs)
      .then((text) => {
        finalAtMs = performance.now()
        return text
      })
    // Avoid an unhandled rejection if TTS throws before the final promise is awaited.
    finalPromise.catch(() => undefined)

    const ttsCalledAtMs = performance.now()
    await leg.speaker.sendTextToTTS(phrase)
    const firstStartMs = leg.agentStartTimesMs[startsBaseline]
    if (firstStartMs != null) result.ttsStartLatencyMs = firstStartMs - ttsCalledAtMs

    await leg.speakerEndLatch
      .waitAfterCount(endsBaseline, SPEAKER_END_WAIT_MS)
      .catch(() => undefined)
    const speakerEndMs = leg.agentEndTimesMs[endsBaseline] ?? performance.now()

    const [text] = await Promise.all([
      finalPromise,
      streamSilence(leg.agentOut, timing.postTtsSilenceS),
    ])
    result.finalText = text
    if (finalAtMs != null) result.finalLatencyMs = Math.max(0, finalAtMs - speakerEndMs)
  } catch (error) {
    result.error = errorMessage(error)
  }
  return result
}

async function runLeg(
  leg: LoadLeg,
  turns: number,
  timing: { timeoutMs: number; finalizeWaitMs: number; postTtsSilenceS: number },
): Promise<LoadTurnResult[]> {
  await sleepMs((leg.index * 137) % 500)
  const results: LoadTurnResult[] = []
  for (let turn = 0; turn < turns; turn++) {
    const result = await runTurn(leg, turn, timing)
    results.push(result)
    if (result.error) {
      // The collector may still hold an open wait; do not stack further turns on it.
      for (let skipped = turn + 1; skipped < turns; skipped++) {
        const { keyword } = LEG_CATALOG[(leg.index + skipped) % LEG_CATALOG.length]!
        results.push({
          legId: leg.legId,
          turn: skipped + 1,
          keyword,
          finalText: null,
          finalLatencyMs: null,
          ttsStartLatencyMs: null,
          error: `skipped after turn ${turn + 1} failed`,
        })
      }
      break
    }
  }
  return results
}

function round(value: number | null, digits = 2): number | null {
  if (value == null) return null
  const f = 10 ** digits
  return Math.round(value * f) / f
}

type RunOutcome = {
  legs: number
  turns: number
  evaluation: LoadRunEvaluation
  results: LoadTurnResult[]
  metrics: Record<string, number | null>
}

async function runLoad(
  config: VoiceAgentConfig,
  legCount: number,
  turns: number,
  turnTimeoutMs: number,
  verbose: boolean,
): Promise<RunOutcome> {
  const timing = {
    timeoutMs: turnTimeoutMs,
    finalizeWaitMs: sttFinalizeWaitMs(config),
    postTtsSilenceS: postTtsSilenceSeconds(config),
  }
  const legs: LoadLeg[] = []
  try {
    const settled = await Promise.allSettled(
      Array.from({ length: legCount }, (_, i) => setupLeg(config, i, verbose)),
    )
    for (const entry of settled) {
      if (entry.status === 'fulfilled') legs.push(entry.value)
    }
    const rejected = settled.find((entry) => entry.status === 'rejected')
    if (rejected && rejected.status === 'rejected') throw rejected.reason

    const delay = monitorEventLoopDelay({ resolution: 1 })
    delay.enable()
    const cpuStart = process.cpuUsage()
    const wallStartMs = performance.now()

    const perLeg = await Promise.all(legs.map((leg) => runLeg(leg, turns, timing)))

    const wallS = (performance.now() - wallStartMs) / 1000
    const cpu = process.cpuUsage(cpuStart)
    delay.disable()
    const cpuS = (cpu.user + cpu.system) / 1e6
    const results = perLeg.flat()
    const evaluation = evaluateLoadRun(results)

    return {
      legs: legCount,
      turns,
      evaluation,
      results,
      metrics: {
        final_latency_ms_p50: round(evaluation.finalLatencyMs.p50),
        final_latency_ms_p95: round(evaluation.finalLatencyMs.p95),
        tts_start_latency_ms_p50: round(evaluation.ttsStartLatencyMs.p50),
        tts_start_latency_ms_p95: round(evaluation.ttsStartLatencyMs.p95),
        turn_success_rate: round(evaluation.turnSuccessRate, 4),
        cpu_s_per_session_min: round(cpuS / legCount / (wallS / 60)),
        event_loop_lag_ms_p99: round(delay.percentile(99) / 1e6),
        rss_mb_end: round(process.memoryUsage().rss / (1024 * 1024)),
        wall_s: round(wallS),
      },
    }
  } finally {
    await teardownLegs(legs)
  }
}

function printProbe(outcome: RunOutcome, extra: Record<string, unknown> = {}): void {
  console.log(
    `perf_probe ${JSON.stringify({
      name: 'roundtrip-load',
      legs: outcome.legs,
      turns: outcome.turns,
      ...extra,
      metrics: outcome.metrics,
    })}`,
  )
}

function failFunctional(outcome: RunOutcome, reason: string): never {
  return exitSherpaRoundtripFailure({
    reason,
    failures: outcome.evaluation.failures,
    legs: [],
  })
}

async function main(): Promise<void> {
  const legCount = envPositiveInt('SHERPA_LOAD_LEGS', DEFAULT_LEGS)
  const turns = envPositiveInt('SHERPA_LOAD_TURNS', DEFAULT_TURNS)
  const capacityMode = process.env.SHERPA_LOAD_CAPACITY === '1'
  const sloDeltaMs = envPositiveInt('SHERPA_LOAD_SLO_DELTA_MS', DEFAULT_SLO_DELTA_MS)
  const maxLegs = envPositiveInt('SHERPA_LOAD_CAPACITY_MAX_LEGS', DEFAULT_CAPACITY_MAX_LEGS)
  const turnTimeoutMs = envPositiveInt('SHERPA_LOAD_TURN_TIMEOUT_MS', DEFAULT_TURN_TIMEOUT_MS)
  const verbose = process.env.SHERPA_COUNTING_VERBOSE === '1'

  const { config, label, sttModelPath, ttsModelPath } = resolveRoundtripVoiceConfig()

  // Budget: setup + (per turn: TTS + trailing silence + STT finalize) for each step.
  const perTurnMs = postTtsSilenceSeconds(config) * 1000 + 30_000
  const stepBudgetMs = 60_000 + turns * perTurnMs
  const stepCount = capacityMode
    ? Math.max(1, CAPACITY_STEPS.filter((n) => n <= maxLegs).length)
    : 1
  installRoundtripWallClockTimeout(stepCount * stepBudgetMs)

  console.log(
    `=== Sherpa load roundtrip (${capacityMode ? 'capacity mode' : `${legCount} legs`}, ${turns} turns/leg) ===`,
  )
  console.log(`Pipeline: ${label}`)
  console.log(
    `Listener: gateStt=${config.vad?.gateStt !== false}  minSilence=${config.vad?.minSilenceDurationMs ?? VOICE_AGENT_VAD_PRESET.minSilenceDurationMs}ms`,
  )
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log(
    `SHERPA_POOL_MAX_CONCURRENT_TTS=${process.env.SHERPA_POOL_MAX_CONCURRENT_TTS ?? '(default)'}`,
  )
  console.log('')

  if (!capacityMode) {
    const outcome = await runLoad(config, legCount, turns, turnTimeoutMs, verbose)
    printProbe(outcome)
    if (outcome.evaluation.failures.length > 0) {
      failFunctional(outcome, 'load roundtrip failed')
    }
    console.log('')
    console.log('=== PASS ===')
    process.exit(0)
  }

  let step = 2
  let maxLegsAtSlo = 0
  let baselineP95Ms: number | null = null
  for (;;) {
    const outcome = await runLoad(config, step, turns, turnTimeoutMs, verbose)
    let passed: boolean
    if (baselineP95Ms === null) {
      // The 2-leg step is judged only functionally; its p95 is the baseline for later steps.
      if (outcome.evaluation.failures.length > 0) {
        failFunctional(outcome, 'capacity mode: 2-leg step failed functionally')
      }
      baselineP95Ms = outcome.evaluation.finalLatencyMs.p95
      if (baselineP95Ms === null) {
        failFunctional(outcome, 'capacity mode: 2-leg step produced no final latency baseline')
      }
      passed = true
    } else {
      passed = capacityStepPasses(outcome.evaluation, baselineP95Ms, sloDeltaMs)
    }
    printProbe(outcome, {
      step_legs: step,
      passed,
      baseline_final_p95_ms: round(baselineP95Ms),
      slo_delta_ms: sloDeltaMs,
    })
    if (!passed) break
    maxLegsAtSlo = step
    const next = nextCapacityStep(step, maxLegs)
    if (next === step) break
    step = next
  }
  console.log(`roundtrip-load capacity max_legs_at_slo=${maxLegsAtSlo}`)
  process.exit(0)
}

main().catch((error: unknown) => {
  exitSherpaRoundtripFailure({
    reason: errorMessage(error),
    failures: [],
    legs: [],
    error,
  })
})
