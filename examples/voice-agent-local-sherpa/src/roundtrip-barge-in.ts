/**
 * E2E semantic barge-in (Sherpa STT + TTS) — loopback with production VAD preset.
 *
 * **UX:** barge-in fires when STT recognizes words (`bargeIn.requireSttPartial`, default true),
 * not on coughs/tones that only trip energy VAD.
 *
 * Phases:
 *   1. Full agent TTS playback (no interrupt).
 *   2. Random tone on user leg mid-playback → must **not** emit `barge_in`; playback ~full length.
 *   3. User-leg Sherpa TTS ("stop now please") mid-playback → must emit `barge_in`; playback truncated.
 *
 * Run:
 *   npm run start:roundtrip-barge-in --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Env: SHERPA_BARGE_IN_PHRASE, SHERPA_BARGE_IN_BARGE_PHRASE, SHERPA_BARGE_IN_DELAY_MS, …
 * Logs every speech event with phase-relative timestamps; see ROUNDTRIP.md § Semantic barge-in E2E.
 */

import type { RemoteAudioTrack } from '@node-webrtc-rust/sdk'
import {
  VoiceAgent,
  VOICE_AGENT_VAD_PRESET,
  SPEECH_EVENT_TYPE,
  type SpeechEventType,
  type VoiceAgentConfig,
} from '@node-webrtc-rust/sdk/voice'
import {
  evaluateBargeUtteranceFinal,
  evaluateBargePathLifecycle,
  DEFAULT_BARGE_PHRASE_MIN_SIMILARITY,
  evaluateSemanticBargeEventOrder,
  evaluateTonePhaseLifecycle,
  formatRecordedSpeechEvent,
  hasUserSpeechFinal,
  logRecordedSpeechEvents,
  phase1BaselineComplete,
  phase2EventsComplete,
  phase3EventsTerminal,
  recordSpeechEvent,
  type RecordedSpeechEvent,
} from './roundtrip-barge-in-helpers.js'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import {
  installRoundtripWallClockTimeout,
  interPhaseSttDrainSeconds,
  postTtsSilenceSeconds,
} from './roundtrip-counting.js'
import { stereoPcmDurationMs, streamSilence, streamTone } from './pcm-relay.js'
import { resolveVoiceConfig } from './resolve-voice-config.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import { logRoundtripSpeechEvent } from './roundtrip-speech-events.js'
import {
  attachRoundtripConnectionLogs,
  logE2ePhase,
  logRoundtripScriptBanner,
  logSignalingReady,
  logVoiceAgentAttach,
} from './roundtrip-topology-log.js'

const DEFAULT_AGENT_PHRASE =
  'The quick brown fox jumps over the lazy dog and then continues speaking for several more seconds so we can interrupt playback.'

const DEFAULT_BARGE_TTS_PHRASE = 'stop now please'

const DEFAULT_BARGE_DELAY_MS = 700
const DEFAULT_TONE_INTERRUPT_S = 1.0
const DEFAULT_MAX_CUT_RATIO = 0.65
const DEFAULT_MIN_FULL_RATIO_AFTER_NOISE = 0.75
const DEFAULT_MAX_PHASE_MS = 25_000
const DEFAULT_WARMUP_S = 0.6

/**
 * One `speechEvents()` pump for the whole E2E — per-phase iterators block on the next
 * `pullSpeechEvent` and orphan pumps steal events after a wall-clock race.
 */
class ListenerSpeechCollector {
  private phase: {
    collected: RecordedSpeechEvent[]
    phaseStartMs: number
    phaseLabel: string
    until: (events: RecordedSpeechEvent[]) => boolean
    finish: () => void
    waiters: Array<{ type: SpeechEventType; resolve: () => void }>
  } | null = null

  constructor(agent: VoiceAgent) {
    void this.pump(agent)
  }

  private notifyWaiters(collected: RecordedSpeechEvent[]): void {
    const phase = this.phase
    if (phase == null) return
    for (const waiter of phase.waiters) {
      if (collected.some((e) => e.type === waiter.type)) {
        waiter.resolve()
      }
    }
    phase.waiters = phase.waiters.filter((w) => !collected.some((e) => e.type === w.type))
  }

  /** Wait for an event in the active phase (e.g. agent_speaking_start before mid-playback barge). */
  waitForEvent(type: SpeechEventType, maxMs: number): Promise<void> {
    const phase = this.phase
    if (phase == null) {
      return Promise.reject(new Error('speech collector has no active phase'))
    }
    if (phase.collected.some((e) => e.type === type)) {
      return Promise.resolve()
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`timed out waiting for ${type} after ${maxMs} ms`)),
        maxMs,
      )
      phase.waiters.push({
        type,
        resolve: () => {
          clearTimeout(timer)
          resolve()
        },
      })
    })
  }

  private async pump(agent: VoiceAgent): Promise<void> {
    try {
      for await (const event of agent.speechEvents()) {
        const active = this.phase
        if (active == null) continue

        logRoundtripSpeechEvent(active.phaseLabel, event)
        const recorded = recordSpeechEvent(active.collected, event, active.phaseStartMs)
        console.log(`[${active.phaseLabel}] event ${formatRecordedSpeechEvent(recorded)}`)
        this.notifyWaiters(active.collected)
        if (active.until(active.collected)) {
          console.log(`[${active.phaseLabel}] terminal event sequence reached`)
          active.finish()
        }
      }
    } catch (error) {
      console.error('[speech-events] stream error:', error)
    }
  }

  collectUntil(params: {
    phaseLabel: string
    maxMs: number
    until: (events: RecordedSpeechEvent[]) => boolean
  }): Promise<RecordedSpeechEvent[]> {
    if (this.phase != null) {
      return Promise.reject(new Error(`speech collector busy during ${this.phase.phaseLabel}`))
    }

    const collected: RecordedSpeechEvent[] = []
    const phaseStartMs = Date.now()

    console.log(`[${params.phaseLabel}] collecting speech events (max ${params.maxMs} ms)`)

    return new Promise((resolve) => {
      let wallTimer: ReturnType<typeof setTimeout> | null = null

      const done = (reason: 'terminal' | 'wall') => {
        if (wallTimer != null) {
          clearTimeout(wallTimer)
          wallTimer = null
        }
        if (this.phase == null) return
        this.phase = null
        if (reason === 'wall') {
          console.error(
            `[${params.phaseLabel}] wall-clock cap ${params.maxMs} ms — returning ${collected.length} events`,
          )
        }
        resolve([...collected])
      }

      this.phase = {
        collected,
        phaseStartMs,
        phaseLabel: params.phaseLabel,
        until: params.until,
        finish: () => done('terminal'),
        waiters: [],
      }
      wallTimer = setTimeout(() => done('wall'), params.maxMs)
    })
  }
}

function listenerVadConfig(base: VoiceAgentConfig): NonNullable<VoiceAgentConfig['vad']> {
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

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

class InboundAudioMeter {
  private totalMs = 0
  private running = false

  reset(): void {
    this.totalMs = 0
  }

  start(track: RemoteAudioTrack): void {
    this.running = true
    void this.loop(track)
  }

  stop(): void {
    this.running = false
  }

  getTotalMs(): number {
    return this.totalMs
  }

  private async loop(track: RemoteAudioTrack): Promise<void> {
    while (this.running) {
      try {
        const pcm = await track.readSample()
        if (pcm.length === 0) {
          await sleep(10)
          continue
        }
        this.totalMs += stereoPcmDurationMs(pcm.length)
      } catch {
        await sleep(20)
      }
    }
  }
}

async function runMidPlaybackInterrupt(params: {
  listener: VoiceAgent
  collector: ListenerSpeechCollector
  agentPhrase: string
  userInbound: RemoteAudioTrack
  delayMs: number
  interrupt: () => Promise<void>
  maxPhaseMs: number
  untilEvents: (events: RecordedSpeechEvent[]) => boolean
  phaseLabel: string
}): Promise<{ receivedMs: number; events: RecordedSpeechEvent[] }> {
  const meter = new InboundAudioMeter()
  meter.start(params.userInbound)

  const eventsPromise = params.collector.collectUntil({
    phaseLabel: params.phaseLabel,
    maxMs: params.maxPhaseMs,
    until: params.untilEvents,
  })

  console.log(`[${params.phaseLabel}] agent TTS queued (${params.agentPhrase.length} chars)`)
  const ttsDone = params.listener.sendTextToTTS(params.agentPhrase)

  await params.collector.waitForEvent(SPEECH_EVENT_TYPE.agentSpeakingStart, params.maxPhaseMs)
  await sleep(params.delayMs)
  console.log(`[${params.phaseLabel}] interrupt at agent_speaking_start + ${params.delayMs} ms`)
  await params.interrupt()

  const events = await eventsPromise
  meter.stop()
  const receivedMs = meter.getTotalMs()
  await ttsDone
  await sleep(200)

  console.log(
    `[${params.phaseLabel}] done: userInbound=${receivedMs} ms audio, ${events.length} events`,
  )
  logRecordedSpeechEvents(events, params.phaseLabel)

  return { receivedMs, events }
}

async function main(): Promise<void> {
  installRoundtripWallClockTimeout(120_000)

  const agentPhrase =
    process.env.SHERPA_BARGE_IN_PHRASE?.trim() ||
    process.argv.slice(2).join(' ').trim() ||
    DEFAULT_AGENT_PHRASE
  const bargePhrase = process.env.SHERPA_BARGE_IN_BARGE_PHRASE?.trim() || DEFAULT_BARGE_TTS_PHRASE
  const bargeDelayMs = Number(process.env.SHERPA_BARGE_IN_DELAY_MS ?? DEFAULT_BARGE_DELAY_MS)
  const toneInterruptS = Number(process.env.SHERPA_BARGE_IN_TONE_S ?? DEFAULT_TONE_INTERRUPT_S)
  const maxCutRatio = Number(process.env.SHERPA_BARGE_IN_MAX_RATIO ?? DEFAULT_MAX_CUT_RATIO)
  const minFullAfterNoise = Number(
    process.env.SHERPA_BARGE_IN_MIN_FULL_AFTER_NOISE ?? DEFAULT_MIN_FULL_RATIO_AFTER_NOISE,
  )
  const maxPhaseMs = Number(process.env.SHERPA_BARGE_IN_TIMEOUT_MS ?? DEFAULT_MAX_PHASE_MS)
  const bargeMinSimilarity = Number(
    process.env.SHERPA_BARGE_IN_MIN_SIMILARITY ?? DEFAULT_BARGE_PHRASE_MIN_SIMILARITY,
  )

  const { config, label, sttModelPath, ttsModelPath } = resolveVoiceConfig()

  console.log('=== Sherpa semantic barge-in E2E ===')
  logRoundtripScriptBanner({
    script: 'roundtrip-barge-in',
    pipeline: label,
    extra: [
      `requireSttPartial=${config.vad?.bargeIn?.requireSttPartial !== false}`,
      `bargeDelayMs=${bargeDelayMs} maxPhaseMs=${maxPhaseMs}`,
    ],
  })
  console.log(`Pipeline: ${label}`)
  console.log(`requireSttPartial=${config.vad?.bargeIn?.requireSttPartial !== false}`)
  console.log(`Agent phrase: ${agentPhrase.slice(0, 80)}…`)
  console.log(`Barge phrase (TTS): "${bargePhrase}"`)
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log(`bargeDelayMs=${bargeDelayMs} toneInterruptS=${toneInterruptS}`)
  console.log(
    `maxCutRatio=${maxCutRatio} minFullAfterNoise=${minFullAfterNoise} maxPhaseMs=${maxPhaseMs}`,
  )
  console.log(`bargeMinSimilarity=${bargeMinSimilarity}`)
  console.log('')

  const { server, agentPc, userPc, agentOut, userInbound, userOut, agentInbound, cleanup } =
    await createBidirectionalLoopback()

  logSignalingReady({ port: server.port })
  attachRoundtripConnectionLogs({ agentPc, userPc })

  const listener = new VoiceAgent({
    stt: config.stt,
    tts: config.tts,
    events: { mode: 'stream' },
    vad: listenerVadConfig(config),
  })

  const userSpeaker = new VoiceAgent({
    tts: config.tts,
    events: { mode: 'stream' },
    vad: { enabled: false },
  })

  await listener.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  logVoiceAgentAttach({
    role: 'listener',
    label: 'listener VoiceAgent (STT+VAD+barge on agent-pc)',
    inboundTrack: 'agentInbound ← user-pc RTP',
    outboundTrack: 'agentOut → user-pc',
  })
  await userSpeaker.attach({
    inboundTrack: userInbound,
    outboundTrack: userOut,
  })
  logVoiceAgentAttach({
    role: 'user-sim',
    label: 'user simulator VoiceAgent (TTS only on user-pc)',
    inboundTrack: 'userInbound ← agent-pc RTP',
    outboundTrack: 'userOut → agent-pc',
  })
  await listener.start()
  await userSpeaker.start()

  await streamSilence(agentOut, DEFAULT_WARMUP_S)
  await streamSilence(userOut, DEFAULT_WARMUP_S)

  const failures: string[] = []
  const speechCollector = new ListenerSpeechCollector(listener)

  logE2ePhase({ phase: 'Phase 1', detail: 'full agent TTS playback (no interrupt)' })
  console.log('--- Phase 1: full playback (no interrupt) ---')
  const phase1Label = 'Phase 1'
  const meterFull = new InboundAudioMeter()
  meterFull.start(userInbound)
  console.log(`[${phase1Label}] agent TTS queued (${agentPhrase.length} chars)`)
  const phase1EventsPromise = speechCollector.collectUntil({
    phaseLabel: phase1Label,
    maxMs: maxPhaseMs,
    until: phase1BaselineComplete,
  })
  await listener.sendTextToTTS(agentPhrase)
  const phase1Events = await phase1EventsPromise
  meterFull.stop()
  const fullMs = meterFull.getTotalMs()
  console.log(`[${phase1Label}] userInbound=${fullMs} ms (full baseline)`)
  logRecordedSpeechEvents(phase1Events, phase1Label)

  if (!phase1BaselineComplete(phase1Events)) {
    failures.push('Phase 1: missing agent_speaking_start → agent_speaking_end baseline')
  }
  if (fullMs < 500) {
    failures.push('Phase 1: full playback too short — check loopback / TTS')
  }

  await streamSilence(agentOut, 0.5)
  await streamSilence(userOut, 0.5)

  logE2ePhase({ phase: 'Phase 2', detail: 'tone on user-pc mid-playback — must NOT barge' })
  console.log('--- Phase 2: tone mid-playback (must NOT barge) ---')
  const noiseResult = await runMidPlaybackInterrupt({
    listener,
    collector: speechCollector,
    agentPhrase,
    userInbound,
    delayMs: bargeDelayMs,
    interrupt: async () => {
      console.log(`[Phase 2] user tone ${toneInterruptS}s @ 440 Hz (non-speech)`)
      await streamTone(userOut, toneInterruptS, 440)
    },
    maxPhaseMs,
    untilEvents: phase2EventsComplete,
    phaseLabel: 'Phase 2',
  })
  const noiseRatio = noiseResult.receivedMs / fullMs
  const noiseBarges = noiseResult.events.filter((e) => e.type === SPEECH_EVENT_TYPE.bargeIn).length
  const toneEval = evaluateTonePhaseLifecycle({
    events: noiseResult.events,
    bargeCount: noiseBarges,
    label: 'Phase 2',
  })
  console.log(
    `Pre-interrupt received: ${noiseResult.receivedMs} ms (${(noiseRatio * 100).toFixed(0)}% of full)`,
  )
  console.log(`barge_in count during tone: ${noiseBarges}`)

  if (!toneEval.passed) {
    failures.push(...toneEval.failures.map((f) => `Phase 2: ${f}`))
  }
  if (noiseRatio < minFullAfterNoise) {
    failures.push(
      `Phase 2: playback truncated by noise (${(noiseRatio * 100).toFixed(0)}% < ${(minFullAfterNoise * 100).toFixed(0)}%)`,
    )
  }

  const interPhaseDrainS = interPhaseSttDrainSeconds(config)
  console.log(
    `[between Phase 2→3] draining listener STT (${interPhaseDrainS.toFixed(1)}s silence — minSilence + gate hold + endpoint tail)`,
  )
  await streamSilence(agentOut, 0.3)
  await streamSilence(userOut, interPhaseDrainS)

  logE2ePhase({ phase: 'Phase 3', detail: 'user TTS barge phrase on user-pc — must barge' })
  console.log('--- Phase 3: user TTS barge phrase mid-playback (must barge) ---')
  const postSilenceS = postTtsSilenceSeconds(config)
  const speechResult = await runMidPlaybackInterrupt({
    listener,
    collector: speechCollector,
    agentPhrase,
    userInbound,
    delayMs: bargeDelayMs,
    interrupt: async () => {
      console.log(`[Phase 3] user Sherpa TTS barge: "${bargePhrase}"`)
      await userSpeaker.sendTextToTTS(bargePhrase)
      console.log(
        `[Phase 3] trailing silence ${postSilenceS.toFixed(1)}s (parallel with event collection)`,
      )
      // Real-time PCM must run while the collector phase is active — do not await before collect ends.
      void streamSilence(userOut, postSilenceS)
    },
    maxPhaseMs,
    untilEvents: phase3EventsTerminal,
    phaseLabel: 'Phase 3',
  })

  let phase3Events = speechResult.events
  if (!hasUserSpeechFinal(phase3Events)) {
    console.log(
      `[Phase 3 finalize] trailing silence ${postSilenceS.toFixed(1)}s + collect (parallel)`,
    )
    const [finalizeEvents] = await Promise.all([
      speechCollector.collectUntil({
        phaseLabel: 'Phase 3 finalize',
        maxMs: 15_000,
        until: hasUserSpeechFinal,
      }),
      streamSilence(userOut, postSilenceS),
    ])
    phase3Events = [...phase3Events, ...finalizeEvents]
    logRecordedSpeechEvents(finalizeEvents, 'Phase 3 finalize')
  }

  const speechRatio = speechResult.receivedMs / fullMs
  const orderEval = evaluateSemanticBargeEventOrder({
    events: phase3Events,
    expectedPhrase: bargePhrase,
    label: 'Phase 3',
  })
  console.log(
    `Pre-barge received: ${speechResult.receivedMs} ms (${(speechRatio * 100).toFixed(0)}% of full)`,
  )
  if (
    orderEval.partialAtMs != null &&
    orderEval.bargeAtMs != null &&
    orderEval.agentEndAtMs != null
  ) {
    console.log(
      `Event order (ms from phase start): agent_speaking_start=${orderEval.agentStartAtMs} → ` +
        `partial=${orderEval.partialAtMs} → barge_in=${orderEval.bargeAtMs} → agent_speaking_end=${orderEval.agentEndAtMs}`,
    )
  }

  if (!orderEval.passed) {
    failures.push(...orderEval.failures)
  }

  const lifecycleEval = evaluateBargePathLifecycle({
    events: phase3Events,
    label: 'Phase 3',
  })
  if (!lifecycleEval.passed) {
    failures.push(...lifecycleEval.failures)
  }

  const utteranceEval = evaluateBargeUtteranceFinal({
    events: phase3Events,
    expectedPhrase: bargePhrase,
    minSimilarity: bargeMinSimilarity,
    label: 'Phase 3',
  })
  if (utteranceEval.recognized) {
    console.log(
      `Barge utterance recognized: "${utteranceEval.recognized}" (similarity ${(utteranceEval.similarity * 100).toFixed(0)}%)`,
    )
  }
  if (utteranceEval.endToFinalGapMs != null) {
    console.log(`user_speaking_end → user_speech_final: ${utteranceEval.endToFinalGapMs} ms`)
  }
  if (!utteranceEval.passed) {
    failures.push(...utteranceEval.failures)
  }

  if (speechRatio >= maxCutRatio) {
    failures.push(
      `Phase 3: playback not truncated enough (${(speechRatio * 100).toFixed(0)}% >= ${(maxCutRatio * 100).toFixed(0)}%)`,
    )
  }

  await listener.stop().catch(() => undefined)
  await userSpeaker.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  if (failures.length > 0) {
    exitSherpaRoundtripFailure({
      reason: 'semantic barge-in assertions failed',
      failures,
    })
  }

  console.log('\nSemantic barge-in E2E OK — tone ignored, spoken phrase interrupted agent TTS.')
  process.exit(0)
}

main().catch((error: unknown) => {
  exitSherpaRoundtripFailure({
    reason: 'uncaught error',
    error,
  })
})
