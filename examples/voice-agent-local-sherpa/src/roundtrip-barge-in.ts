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
import { VoiceAgent, VOICE_AGENT_VAD_PRESET, SPEECH_EVENT_TYPE } from '@node-webrtc-rust/sdk/voice'
import type { VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

import {
  evaluateSemanticBargeEventOrder,
  evaluateToneMustNotBarge,
  formatRecordedSpeechEvent,
  logRecordedSpeechEvents,
  phase1BaselineComplete,
  phase2EventsComplete,
  phase3EventsTerminal,
  recordSpeechEvent,
  type RecordedSpeechEvent,
} from './roundtrip-barge-in-helpers.js'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { stereoPcmDurationMs, streamSilence, streamTone } from './pcm-relay.js'
import { resolveVoiceConfig } from './resolve-voice-config.js'

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
 * Collect listener speech events until `until(events)` or `maxMs` (safety cap).
 * Only one `speechEvents()` consumer may run per VoiceAgent at a time.
 */
function collectSpeechEventsUntil(params: {
  agent: VoiceAgent
  phaseLabel: string
  maxMs: number
  until: (events: RecordedSpeechEvent[]) => boolean
}): Promise<RecordedSpeechEvent[]> {
  const collected: RecordedSpeechEvent[] = []
  const startedAtMs = Date.now()
  const doneAt = startedAtMs + params.maxMs

  console.log(`[${params.phaseLabel}] collecting speech events (max ${params.maxMs} ms)`)

  return (async () => {
    for await (const event of params.agent.speechEvents()) {
      if (Date.now() > doneAt) break
      const recorded = recordSpeechEvent(collected, event, startedAtMs)
      console.log(`[${params.phaseLabel}] event ${formatRecordedSpeechEvent(recorded)}`)
      if (params.until(collected)) {
        console.log(`[${params.phaseLabel}] terminal event sequence reached`)
        break
      }
    }
    return collected
  })()
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

  const eventsPromise = collectSpeechEventsUntil({
    agent: params.listener,
    phaseLabel: params.phaseLabel,
    maxMs: params.maxPhaseMs,
    until: params.untilEvents,
  })

  console.log(`[${params.phaseLabel}] agent TTS queued (${params.agentPhrase.length} chars)`)
  const ttsDone = params.listener.sendTextToTTS(params.agentPhrase)

  await sleep(params.delayMs)
  console.log(`[${params.phaseLabel}] interrupt at +${params.delayMs} ms`)
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

  const { config, label, sttModelPath, ttsModelPath } = resolveVoiceConfig()

  console.log('=== Sherpa semantic barge-in E2E ===')
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
  console.log('')

  const { agentOut, userInbound, userOut, agentInbound, cleanup } =
    await createBidirectionalLoopback()

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
  await userSpeaker.attach({
    inboundTrack: userInbound,
    outboundTrack: userOut,
  })
  await listener.start()
  await userSpeaker.start()

  await streamSilence(agentOut, DEFAULT_WARMUP_S)
  await streamSilence(userOut, DEFAULT_WARMUP_S)

  const failures: string[] = []

  console.log('--- Phase 1: full playback (no interrupt) ---')
  const phase1Label = 'Phase 1'
  const meterFull = new InboundAudioMeter()
  meterFull.start(userInbound)
  console.log(`[${phase1Label}] agent TTS queued (${agentPhrase.length} chars)`)
  const phase1EventsPromise = collectSpeechEventsUntil({
    agent: listener,
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

  console.log('')
  console.log('--- Phase 2: tone mid-playback (must NOT barge) ---')
  const noiseResult = await runMidPlaybackInterrupt({
    listener,
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
  const toneEval = evaluateToneMustNotBarge({ events: noiseResult.events, bargeCount: noiseBarges })
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

  await streamSilence(agentOut, 0.5)
  await streamSilence(userOut, 0.5)

  console.log('')
  console.log('--- Phase 3: user TTS barge phrase mid-playback (must barge) ---')
  const speechResult = await runMidPlaybackInterrupt({
    listener,
    agentPhrase,
    userInbound,
    delayMs: bargeDelayMs,
    interrupt: async () => {
      console.log(`[Phase 3] user Sherpa TTS barge: "${bargePhrase}"`)
      await userSpeaker.sendTextToTTS(bargePhrase)
      void streamSilence(userOut, 1.0)
    },
    maxPhaseMs,
    untilEvents: phase3EventsTerminal,
    phaseLabel: 'Phase 3',
  })
  const speechRatio = speechResult.receivedMs / fullMs
  const orderEval = evaluateSemanticBargeEventOrder({
    events: speechResult.events,
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
  if (speechRatio >= maxCutRatio) {
    failures.push(
      `Phase 3: playback not truncated enough (${(speechRatio * 100).toFixed(0)}% >= ${(maxCutRatio * 100).toFixed(0)}%)`,
    )
  }

  await listener.stop().catch(() => undefined)
  await userSpeaker.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  if (failures.length > 0) {
    console.error('\nSemantic barge-in E2E FAILED:')
    for (const f of failures) console.error(`  - ${f}`)
    process.exit(1)
  }

  console.log('\nSemantic barge-in E2E OK — tone ignored, spoken phrase interrupted agent TTS.')
  process.exit(0)
}

main().catch((error: unknown) => {
  console.error(error)
  process.exit(1)
})
