/**
 * Sherpa roundtrip — STT "no partial" safety net (C1) timeouts over real loopback WebRTC.
 *
 * Contract (`sttListenTimeoutMs` = 4 s, hard timeout unset = 3 x 4 s = 12 s):
 * - Frames keep flowing (silence after a noise burst): the stream closes through the normal endpoint
 *   (minSilence + gate hold, about 2.3 s here) and never waits for the 12 s bound.
 * - No PCM reaches the agent while VAD is in speech: only the 12 s hard timeout closes the stream.
 * - A 4.5 s reader-side gap mid-phrase must not close the stream; the words after the gap are heard.
 *
 * Phase A is unchanged behavior: 0.9.48 and 0.10 both close at about 2.3 s here. B (4 s on 0.9.48 vs
 * about 11.8 s now) and C (false close and truncated final on 0.9.48) separate old from new.
 *
 * Phases (each preceded by 3 s of silence frames):
 *   A  600 ms noise, then 10 s of silence frames.
 *   B  400 ms of the same noise, then nothing for 16 s.
 *   C  TTS "one … ten" rendered once into PCM; 1.6 s, 4.5 s of nothing, the rest, silence.
 *
 * The TTS PCM for phase C comes from a second, throwaway loopback with a speaker VoiceAgent; the
 * decoded PCM the far peer receives (`readSample` on its inbound track) is captured.
 *
 * Run:
 *   npm run start:roundtrip-c1-timeouts --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 */

import type { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { PCM_FRAME_DURATION_MS, PCM_FULL_FRAME_BYTES } from '../../shared/pcm-streaming.js'
import { stereoPcmDurationMs, streamSilence } from './pcm-relay.js'
import { resolveRoundtripVoiceConfig } from './resolve-voice-config.js'
import {
  createNoiseFrames,
  evaluatePhaseA,
  evaluatePhaseB,
  evaluatePhaseC,
  type C1PhaseResult,
  type C1SmokeEvent,
} from './roundtrip-c1-timeouts-helpers.js'
import {
  AgentSpeakingEndLatch,
  DEFAULT_COUNTING_PHRASE_ONE_TO_TEN,
  installRoundtripWallClockTimeout,
  ListenerUtteranceCollector,
  postTtsSilenceSeconds,
  startSpeakerSpeechPump,
} from './roundtrip-counting.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import { enableRoundtripSpeechEventLog } from './roundtrip-speech-events.js'

const STT_LISTEN_TIMEOUT_MS = 4000
const WARMUP_S = 0.6
const QUIET_BETWEEN_PHASES_MS = 3000
const NOISE_A_MS = 600
const NOISE_B_WRITTEN_MS = 400
const PHASE_A_SILENCE_MS = 10_000
const PHASE_B_NOTHING_MS = 16_000
const PHASE_C_HEAD_MS = 1600
const PHASE_C_GAP_MS = 4500
const NOISE_SEED = 20_251_014

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

interface CapturedFrame {
  data: Buffer
  durationMs: number
}

/** Writes PCM paced from an anchor: frame n is due at `anchor + sum(previous durations)`. */
class PacedWriter {
  private anchorAt = performance.now()
  private sentMs = 0

  constructor(private readonly track: LocalAudioTrack) {}

  /** Start a new schedule from now (after a pause); never bursts to catch up. */
  reanchor(): void {
    this.anchorAt = performance.now()
    this.sentMs = 0
  }

  async write(data: Buffer, durationMs: number): Promise<void> {
    const waitMs = this.anchorAt + this.sentMs - performance.now()
    if (waitMs > 0) {
      await sleep(waitMs)
    } else if (waitMs < -100) {
      this.reanchor()
    }
    await this.track.writeSample(data, durationMs)
    this.sentMs += durationMs
  }

  async writeFrames(frames: Buffer[]): Promise<void> {
    for (const frame of frames) await this.write(frame, PCM_FRAME_DURATION_MS)
  }

  async silence(ms: number): Promise<void> {
    const silent = Buffer.alloc(PCM_FULL_FRAME_BYTES)
    const count = Math.ceil(ms / PCM_FRAME_DURATION_MS)
    for (let i = 0; i < count; i++) await this.write(silent, PCM_FRAME_DURATION_MS)
  }

  async frames(captured: CapturedFrame[]): Promise<void> {
    for (const f of captured) await this.write(f.data, f.durationMs)
  }
}

/**
 * Render `phrase` once with a speaker VoiceAgent on a throwaway loopback and capture the
 * `writeSample` calls on its outbound track.
 */
async function renderTtsPcm(
  tts: NonNullable<ReturnType<typeof resolveRoundtripVoiceConfig>['config']['tts']>,
  phrase: string,
): Promise<CapturedFrame[]> {
  const { agentOut, agentInbound, userInbound, cleanup } = await createBidirectionalLoopback()
  const speaker = new VoiceAgent({ tts, events: { mode: 'stream' }, vad: { enabled: false } })
  await speaker.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await speaker.start()
  await streamSilence(agentOut, WARMUP_S)

  const latch = new AgentSpeakingEndLatch()
  startSpeakerSpeechPump(speaker, latch)

  // TTS PCM is written natively (not through `writeSample`), so tap what the far peer receives:
  // the decoded PCM on `userInbound`, as `relayRemoteAudioToLocal` reads it.
  const captured: CapturedFrame[] = []
  let capturing = true
  void (async () => {
    while (capturing) {
      try {
        const pcm = await userInbound.readSample()
        if (pcm.length === 0) {
          await sleep(5)
          continue
        }
        if (capturing)
          captured.push({ data: Buffer.from(pcm), durationMs: stereoPcmDurationMs(pcm.length) })
      } catch {
        await sleep(20)
      }
    }
  })()

  const baseline = latch.endEventsSeen()
  console.log(`[c1-smoke] rendering TTS: "${phrase}"`)
  await speaker.sendTextToTTS(phrase)
  await latch.waitAfterCount(baseline, 40_000)
  await sleep(300)
  capturing = false

  await speaker.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)
  return trimSilence(captured)
}

function peak(data: Buffer): number {
  let max = 0
  for (let i = 0; i + 1 < data.length; i += 2) max = Math.max(max, Math.abs(data.readInt16LE(i)))
  return max
}

/** Drop idle frames before the first and after the last audible frame. */
function trimSilence(frames: CapturedFrame[]): CapturedFrame[] {
  const audible = (f: CapturedFrame): boolean => peak(f.data) > 300
  const first = frames.findIndex(audible)
  if (first < 0) return []
  let last = frames.length - 1
  while (last > first && !audible(frames[last]!)) last -= 1
  return frames.slice(first, last + 1)
}

function totalMs(frames: CapturedFrame[]): number {
  return frames.reduce((sum, f) => sum + f.durationMs, 0)
}

function splitAtMs(
  frames: CapturedFrame[],
  headMs: number,
): { head: CapturedFrame[]; tail: CapturedFrame[] } {
  let acc = 0
  let i = 0
  while (i < frames.length && acc < headMs) {
    acc += frames[i]!.durationMs
    i += 1
  }
  return { head: frames.slice(0, i), tail: frames.slice(i) }
}

function summary(phase: string, r: C1PhaseResult): string {
  const nf = r.notFoundAfterMs.length > 0 ? r.notFoundAfterMs.join(',') : 'none'
  return `[c1-smoke] phase=${phase === 'A' ? 'A (flowing frames: normal close)' : phase} notFoundAfterMs=${nf} finals=${JSON.stringify(r.finals)} ${r.passed ? 'PASS' : 'FAIL'}`
}

async function main(): Promise<void> {
  enableRoundtripSpeechEventLog()
  installRoundtripWallClockTimeout(150_000)
  const phrase = DEFAULT_COUNTING_PHRASE_ONE_TO_TEN

  const { config, label, sttModelPath, ttsModelPath } = resolveRoundtripVoiceConfig()
  if (config.tts == null) throw new Error('resolveRoundtripVoiceConfig returned no tts config')

  console.log('=== Sherpa C1 timeouts roundtrip ===')
  console.log(`Pipeline: ${label}`)
  console.log(`sttListenTimeoutMs=${STT_LISTEN_TIMEOUT_MS} (hard timeout unset = 3x)`)
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log('')

  const ttsFrames = await renderTtsPcm(config.tts, phrase)
  const ttsMs = totalMs(ttsFrames)
  console.log(`[c1-smoke] TTS rendered: ${ttsFrames.length} writes, ${ttsMs} ms`)

  const { agentOut, userInbound, userOut, cleanup } = await createBidirectionalLoopback()
  const listener = new VoiceAgent({
    stt: config.stt,
    events: { mode: 'stream' },
    vad: { ...config.vad, sttListenTimeoutMs: STT_LISTEN_TIMEOUT_MS },
  })
  await listener.attach({ inboundTrack: userInbound, outboundTrack: userOut })
  await listener.start()

  const collector = new ListenerUtteranceCollector(listener, { value: false }, false)
  collector.startPump()

  const writer = new PacedWriter(agentOut)
  await streamSilence(agentOut, WARMUP_S)
  writer.reanchor()

  const failures: string[] = []
  const results: C1PhaseResult[] = []
  const noise = (ms: number) => createNoiseFrames(ms, NOISE_SEED)

  const finish = (phase: string, r: C1PhaseResult, events: C1SmokeEvent[]): void => {
    console.log(summary(phase, r))
    results.push(r)
    if (!r.passed) {
      failures.push(...r.failures)
      failures.push(
        `${phase}: events ${events.map((e) => `${e.type}@${e.atMs}`).join(' ') || '(none)'}`,
      )
    }
  }

  try {
    // Phase A: frames keep flowing after the noise burst.
    await writer.silence(QUIET_BETWEEN_PHASES_MS)
    collector.startEventRecording()
    const aStart = Date.now()
    await writer.writeFrames(noise(NOISE_A_MS))
    const aNoiseEndAtMs = Date.now() - aStart
    await writer.silence(PHASE_A_SILENCE_MS)
    const aEvents = collector.stopEventRecording()
    finish('A', evaluatePhaseA({ events: aEvents, noiseEndAtMs: aNoiseEndAtMs }), aEvents)

    // Phase B: the sender stops mid-speech and writes nothing.
    await writer.silence(QUIET_BETWEEN_PHASES_MS)
    collector.startEventRecording()
    const bStart = Date.now()
    await writer.writeFrames(noise(NOISE_B_WRITTEN_MS))
    const bStopAtMs = Date.now() - bStart
    await sleep(PHASE_B_NOTHING_MS)
    const bEvents = collector.stopEventRecording()
    writer.reanchor()
    finish('B', evaluatePhaseB({ events: bEvents, stopAtMs: bStopAtMs }), bEvents)

    // Phase C: reader-side gap in the middle of a phrase.
    await writer.silence(QUIET_BETWEEN_PHASES_MS)
    if (ttsMs < PHASE_C_HEAD_MS + 1000) {
      throw new Error(`rendered TTS too short for phase C: ${ttsMs} ms`)
    }
    const { head, tail } = splitAtMs(ttsFrames, PHASE_C_HEAD_MS)
    collector.resetStatsForUtterance()
    collector.startEventRecording()
    await writer.frames(head)
    await sleep(PHASE_C_GAP_MS)
    writer.reanchor()
    await writer.frames(tail)
    await writer.silence(Math.max(QUIET_BETWEEN_PHASES_MS, postTtsSilenceSeconds(config) * 1000))
    // Keep silence flowing until a final shows up (bounded) so a slow finalize is not a failure.
    for (let i = 0; i < 8 && collector.stats.finals.length === 0; i++) {
      await writer.silence(1000)
    }
    const cEvents = collector.stopEventRecording()
    finish('C', evaluatePhaseC({ events: cEvents }), cEvents)
  } catch (error) {
    await listener.stop().catch(() => undefined)
    await cleanup().catch(() => undefined)
    exitSherpaRoundtripFailure({ reason: 'c1 timeouts smoke aborted', failures, error })
  }

  await listener.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  if (failures.length > 0) {
    exitSherpaRoundtripFailure({
      reason: 'C1 timeout assertions failed',
      failures,
      legs: [{ label: 'listener', stats: collector.stats }],
    })
  }

  console.log(
    '\nC1 timeouts roundtrip OK — 4 s media-time and 12 s hard timeout behave as documented.',
  )
  process.exit(0)
}

const isMain = process.argv[1]?.endsWith('roundtrip-c1-timeouts.ts') === true

if (isMain) {
  main().catch((error: unknown) => {
    exitSherpaRoundtripFailure({
      reason: 'uncaught error',
      error,
    })
  })
}
