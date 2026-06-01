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
 * See ROUNDTRIP.md § Semantic barge-in E2E.
 */

import type { LocalAudioTrack, RemoteAudioTrack } from '@node-webrtc-rust/sdk'
import { VoiceAgent, VOICE_AGENT_VAD_PRESET } from '@node-webrtc-rust/sdk/voice'
import type { SpeechEvent, VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

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
const DEFAULT_WARMUP_S = 0.6

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

function countBargeInEvents(events: SpeechEvent[]): number {
  return events.filter((e) => e.type === 'barge_in').length
}

async function collectSpeechEventsDuring(
  agent: VoiceAgent,
  durationMs: number,
  verbose: boolean,
): Promise<SpeechEvent[]> {
  const collected: SpeechEvent[] = []
  const doneAt = Date.now() + durationMs

  const pump = (async () => {
    for await (const event of agent.speechEvents()) {
      if (Date.now() > doneAt) break
      collected.push(event)
      if (verbose) {
        const extra = event.text ? ` ${JSON.stringify(event.text)}` : ''
        console.log(`[listener] ${event.type}${extra}`)
      }
    }
  })()

  await sleep(durationMs)
  return collected
}

async function runMidPlaybackInterrupt(params: {
  listener: VoiceAgent
  agentPhrase: string
  userInbound: RemoteAudioTrack
  delayMs: number
  interrupt: () => Promise<void>
  eventWindowMs: number
  verbose: boolean
}): Promise<{ receivedMs: number; events: SpeechEvent[] }> {
  const meter = new InboundAudioMeter()
  meter.start(params.userInbound)

  const eventCollectMs = params.eventWindowMs
  const eventsPromise = collectSpeechEventsDuring(
    params.listener,
    eventCollectMs,
    params.verbose,
  )

  const ttsDone = params.listener.sendTextToTTS(params.agentPhrase)

  await sleep(params.delayMs)
  await params.interrupt()

  const events = await eventsPromise
  meter.stop()
  const receivedMs = meter.getTotalMs()
  await ttsDone
  await sleep(500)

  return { receivedMs, events }
}

async function main(): Promise<void> {
  const agentPhrase =
    process.env.SHERPA_BARGE_IN_PHRASE?.trim() ||
    process.argv.slice(2).join(' ').trim() ||
    DEFAULT_AGENT_PHRASE
  const bargePhrase =
    process.env.SHERPA_BARGE_IN_BARGE_PHRASE?.trim() || DEFAULT_BARGE_TTS_PHRASE
  const bargeDelayMs = Number(process.env.SHERPA_BARGE_IN_DELAY_MS ?? DEFAULT_BARGE_DELAY_MS)
  const toneInterruptS = Number(
    process.env.SHERPA_BARGE_IN_TONE_S ?? DEFAULT_TONE_INTERRUPT_S,
  )
  const maxCutRatio = Number(process.env.SHERPA_BARGE_IN_MAX_RATIO ?? DEFAULT_MAX_CUT_RATIO)
  const minFullAfterNoise = Number(
    process.env.SHERPA_BARGE_IN_MIN_FULL_AFTER_NOISE ?? DEFAULT_MIN_FULL_RATIO_AFTER_NOISE,
  )
  const verbose = process.env.SHERPA_BARGE_IN_VERBOSE === '1'
  const timeoutMs = Number(process.env.SHERPA_BARGE_IN_TIMEOUT_MS ?? 90_000)

  const { config, label, sttModelPath, ttsModelPath } = resolveVoiceConfig()

  console.log('=== Sherpa semantic barge-in E2E ===')
  console.log(`Pipeline: ${label}`)
  console.log(`requireSttPartial=${config.vad?.bargeIn?.requireSttPartial !== false}`)
  console.log(`Agent phrase: ${agentPhrase.slice(0, 80)}…`)
  console.log(`Barge phrase (TTS): "${bargePhrase}"`)
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
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
    inboundTrack: agentInbound,
    outboundTrack: userOut,
  })
  await listener.start()
  await userSpeaker.start()

  await streamSilence(agentOut, DEFAULT_WARMUP_S)
  await streamSilence(userOut, DEFAULT_WARMUP_S)

  const failures: string[] = []

  console.log('--- Phase 1: full playback (no interrupt) ---')
  const meterFull = new InboundAudioMeter()
  meterFull.start(userInbound)
  await listener.sendTextToTTS(agentPhrase)
  await sleep(2)
  meterFull.stop()
  const fullMs = meterFull.getTotalMs()
  console.log(`Received on userInbound: ${fullMs} ms (full)`)

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
      console.log(`[user] Tone ${toneInterruptS}s (non-speech)…`)
      await streamTone(userOut, toneInterruptS, 440)
    },
    eventWindowMs: timeoutMs,
    verbose,
  })
  const noiseRatio = noiseResult.receivedMs / fullMs
  const noiseBarges = countBargeInEvents(noiseResult.events)
  console.log(`Pre-interrupt received: ${noiseResult.receivedMs} ms (${(noiseRatio * 100).toFixed(0)}% of full)`)
  console.log(`barge_in count during tone: ${noiseBarges}`)

  if (noiseBarges > 0) {
    failures.push(`Phase 2: tone must not emit barge_in (saw ${noiseBarges})`)
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
      console.log(`[user] Sherpa TTS barge: "${bargePhrase}"`)
      await userSpeaker.sendTextToTTS(bargePhrase)
    },
    eventWindowMs: timeoutMs,
    verbose,
  })
  const speechRatio = speechResult.receivedMs / fullMs
  const speechBarges = countBargeInEvents(speechResult.events)
  const sawPartial = speechResult.events.some(
    (e) => e.type === 'user_speech_partial' && (e.text?.trim().length ?? 0) >= 2,
  )
  console.log(`Pre-barge received: ${speechResult.receivedMs} ms (${(speechRatio * 100).toFixed(0)}% of full)`)
  console.log(`barge_in=${speechBarges}  user_speech_partial=${sawPartial}`)

  if (speechBarges < 1) {
    failures.push('Phase 3: expected barge_in after STT partial from user TTS phrase')
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
