/**
 * Sherpa roundtrip — a slow STT final must not hold up the next turn or a barge-in.
 *
 * Purpose: real-media smoke (Piper TTS, Sherpa STT, loopback WebRTC) for the STT actor. While a
 * `user_speech_final` is still pending, the echo agent must
 *   - hear the next utterance on time (`user_speaking_start` right after its first voiced frame),
 *   - barge in on time (VAD barge-in and semantic barge-in), and
 *   - deliver finals in utterance order with the matching utterance ids.
 * The inbound path no longer waits for STT, which is why the bounds are 300 ms (VAD min-speech of
 * 200 ms plus transport) and not the 6 s the pending final takes. On the old code the next turn
 * started and barge-in fired about 6 s late.
 *
 * The slow final: this script sets `SPEECH_STT_TEST_FINAL_DELAY_MS=6000`, a test-only knob read by
 * the native STT layer, which delays each final by 6 s. It is set here, inside the script, so it
 * cannot leak into other `start:*` scripts (each npm script is its own process).
 *
 * Process layout: the echo agent and the speaker share this process (the layout of
 * `roundtrip-barge-in`), so the knob is process-wide. That is harmless here: the speaker is a
 * plain `LocalAudioTrack` writer with no STT at all, and its PCM is rendered up front by a
 * throwaway TTS-only agent. Every assertion uses only echo-side events plus speaker-side SEND
 * times of the first voiced frame of each phrase (plus the RMS of audio the speaker receives).
 *
 * Speaker: each phrase is rendered once with Piper into PCM, then written to the track paced
 * from an anchor (frame n due at `anchor + n x 20 ms`, never `write(); sleep(20)`), and there is
 * exactly one writer on the track. Silence frames keep flowing between phrases so the echo VAD
 * sees real silence.
 *
 * Phases (a fresh loopback and echo agent each):
 *   A  next turn on time. P1 "one two three", P2 "four five six" 1.5 s after P1's last
 *      frame, both inside the 6 s hold. Echo: P2 `user_speaking_start` within 300 ms of its first
 *      voiced frame; P1 final (contains "three", id of P1's start) before P2 final (contains
 *      "six", id of P2's start); P2's `user_speaking_start` comes before P1's final (overlap proof);
 *      no `user_stt_not_found`; two replies (two `agent_speaking_start`).
 *   B  VAD barge-in during the slow final (`useVad`, `requireSttPartial: false`). P3 "one two
 *      three"; 1.5 s after P3's last frame (speaker clock) the echo speaks a long filler; 1 s after
 *      the filler starts P4 "four five six" is sent over it. Echo: `barge_in` within 300 ms of P4's first
 *      voiced frame, `agent_speaking_end` within 300 ms of `barge_in`; the speaker receives no
 *      audible agent audio from barge_in + 500 ms to barge_in + 1500 ms; P3 final before P4 final; `barge_in` comes before P3's final.
 *   C  semantic barge-in (`requireSttPartial: true`). Same as B with P5 "one two three" and
 *      P6 "four five six". `barge_in` comes after P6's first `user_speech_partial`, which comes
 *      after P5's `user_speech_final`; P6's final contains "six"; `barge_in` at most
 *      6000 + 1500 ms after P6's first voiced frame; P6's `user_speaking_start` comes before P5's
 *      final (overlap proof). The filler is three sentences (about 14 s) so it outlasts the wait.
 *
 * Overlap proof: `user_speaking_end` is paired with the final, which the knob delays, so the
 * filler is not started from it. The echo host speaks it on the speaker's clock, 1.5 s after the
 * earlier phrase's last frame, and each phase asserts its later event came before the earlier
 * final. Without that a fast result could mean the final was never held back.
 *
 * VAD: minSilence 500 ms and gate hold 500 ms (not the 1300 / 1000 ms of the shared Sherpa
 * config), so a 1.5 s gap between P1 and P2 is a real utterance boundary.
 *
 * APIs demonstrated: `VoiceAgent` with `vad.bargeIn`, `speechEvents()` (`utteranceId`),
 * `sendTextToTTS`, `LocalAudioTrack.writeSample`, `RemoteAudioTrack.readSample`.
 *
 * Run:
 *   npm run start:roundtrip-slow-final --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Failures go through `exitSherpaRoundtripFailure` with a phase tag ("phase A ...") and the echo
 * event timeline; a `[host-suspended]` line marks an inconclusive run (host slept or stalled).
 */

import type { LocalAudioTrack, RemoteAudioTrack } from '@node-webrtc-rust/sdk'
import {
  SPEECH_EVENT_TYPE,
  VOICE_AGENT_VAD_PRESET,
  VoiceAgent,
  type SpeechEvent,
  type VoiceAgentConfig,
} from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { PCM_FRAME_DURATION_MS, PCM_FULL_FRAME_BYTES } from '../../shared/pcm-streaming.js'
import { stereoPcmDurationMs, streamSilence } from './pcm-relay.js'
import { resolveRoundtripVoiceConfig } from './resolve-voice-config.js'
import {
  AgentSpeakingEndLatch,
  installRoundtripWallClockTimeout,
  startSpeakerSpeechPump,
} from './roundtrip-counting.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import {
  FILLER_AFTER_PHRASE_END_MS,
  FILLER_TEXT,
  FINAL_DELAY_MS,
  checkAudioSilencedAfterBarge,
  checkFinalOrder,
  checkNextTurnOnTime,
  checkOverlapsPendingFinal,
  checkNoSttNotFound,
  checkRepliesStarted,
  checkSemanticBargeAfterEarlierFinal,
  checkVadBargeOnTime,
  formatTimeline,
  pcmRms,
  utteranceIdOf,
  type CheckResult,
  type PhraseRef,
  type ReceivedFrame,
  type SlowFinalEvent,
} from './roundtrip-slow-final-helpers.js'
import { logRoundtripSpeechEvent } from './roundtrip-speech-events.js'

type BargeInSetting = NonNullable<NonNullable<VoiceAgentConfig['vad']>['bargeIn']>

const PHRASES = {
  // Number words: Sherpa (kroko) transcribes them reliably from Piper; NATO words like
  // "delta" / "mike" came back as "fell to" / "like". The checks are about timing, order
  // and ids, so each phrase is identified by one distinct key word.
  P1: 'one two three',
  P2: 'four five six',
  P3: 'one two three',
  P4: 'four five six',
  P5: 'one two three',
  P6: 'four five six',
} as const
type PhraseKey = keyof typeof PHRASES

const WARMUP_S = 0.6
const LEAD_IN_MS = 500
/** Silence between P1's last frame and P2's first (inside the 6 s hold). */
const P2_GAP_MS = 1500
/** The interrupting phrase is sent this long after the filler's `agent_speaking_start`. */
const BARGE_AFTER_FILLER_START_MS = 1000
const ECHO_MIN_SILENCE_MS = 500
const ECHO_GATE_HOLD_MS = 500
const PHASE_WAIT_MAX_MS = 45_000

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

interface CapturedFrame {
  data: Buffer
  durationMs: number
}

/**
 * Writes PCM paced from an anchor: frame n is due at `anchor + sum(previous durations)`. One
 * instance is the only writer of its track. `write` returns the wall time just before the frame
 * went to `writeSample`.
 */
class PacedWriter {
  private anchorAt = performance.now()
  private sentMs = 0

  constructor(private readonly track: LocalAudioTrack) {}

  /** Start a new schedule from now (after a pause); never bursts to catch up. */
  reanchor(): void {
    this.anchorAt = performance.now()
    this.sentMs = 0
  }

  async write(data: Buffer, durationMs: number): Promise<number> {
    const waitMs = this.anchorAt + this.sentMs - performance.now()
    if (waitMs > 0) {
      await sleep(waitMs)
    } else if (waitMs < -100) {
      this.reanchor()
    }
    const sentAtWall = Date.now()
    await this.track.writeSample(data, durationMs)
    this.sentMs += durationMs
    return sentAtWall
  }

  async silence(ms: number): Promise<void> {
    const silent = Buffer.alloc(PCM_FULL_FRAME_BYTES)
    const count = Math.ceil(ms / PCM_FRAME_DURATION_MS)
    for (let i = 0; i < count; i++) await this.write(silent, PCM_FRAME_DURATION_MS)
  }

  /** Keep silence frames flowing until `done()` is true; throws after `maxMs`. */
  async silenceUntil(done: () => boolean, maxMs: number, what: string): Promise<void> {
    const silent = Buffer.alloc(PCM_FULL_FRAME_BYTES)
    const deadline = Date.now() + maxMs
    while (!done()) {
      if (Date.now() > deadline) throw new Error(`timed out after ${maxMs} ms waiting for ${what}`)
      await this.write(silent, PCM_FRAME_DURATION_MS)
    }
  }
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

/**
 * Render each phrase once with a speaker VoiceAgent on a throwaway loopback and capture the PCM
 * the far peer receives (`readSample` on its inbound track). TTS PCM is written natively, not
 * through `writeSample`, so this is how the exact frames to replay are obtained.
 */
async function renderPhrases(
  tts: NonNullable<VoiceAgentConfig['tts']>,
  phrases: Record<PhraseKey, string>,
): Promise<Record<PhraseKey, CapturedFrame[]>> {
  const { agentOut, agentInbound, userInbound, cleanup } = await createBidirectionalLoopback()
  const speaker = new VoiceAgent({ tts, events: { mode: 'stream' }, vad: { enabled: false } })
  await speaker.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await speaker.start()
  await streamSilence(agentOut, WARMUP_S)

  const latch = new AgentSpeakingEndLatch()
  startSpeakerSpeechPump(speaker, latch)

  let captured: CapturedFrame[] = []
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

  const out = {} as Record<PhraseKey, CapturedFrame[]>
  for (const key of Object.keys(phrases) as PhraseKey[]) {
    captured = []
    const baseline = latch.endEventsSeen()
    console.log(`[slow-final] rendering ${key}: "${phrases[key]}"`)
    await speaker.sendTextToTTS(phrases[key])
    await latch.waitAfterCount(baseline, 40_000)
    await sleep(300)
    out[key] = trimSilence(captured)
    if (out[key].length === 0) throw new Error(`rendered no audible PCM for ${key}`)
  }
  capturing = false

  await speaker.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)
  return out
}

function echoVadConfig(
  base: VoiceAgentConfig,
  bargeIn: BargeInSetting,
): NonNullable<VoiceAgentConfig['vad']> {
  return {
    ...VOICE_AGENT_VAD_PRESET,
    ...base.vad,
    provider: 'energy',
    threshold: 0.05,
    gateStt: true,
    minSilenceDurationMs: ECHO_MIN_SILENCE_MS,
    sttGateHoldMs: ECHO_GATE_HOLD_MS,
    bargeIn: {
      ...VOICE_AGENT_VAD_PRESET.bargeIn,
      ...base.vad?.bargeIn,
      ...bargeIn,
    },
  }
}

/**
 * One phase: a fresh loopback, an echo agent (STT + TTS + barge-in) on the agent side and a
 * paced PCM writer plus an audio meter on the user side. Records every echo speech event on the
 * phase clock (`clock()` = ms since the phase started).
 */
class PhaseHarness {
  readonly events: SlowFinalEvent[] = []
  readonly frames: ReceivedFrame[] = []
  readonly sends: PhraseRef[] = []
  /** Hook for phase-specific echo behavior; runs for every recorded event. */
  onEvent: (event: SlowFinalEvent, raw: SpeechEvent) => void = () => undefined
  echo!: VoiceAgent
  writer!: PacedWriter
  private t0 = 0
  private metering = false
  private cleanupLoopback: (() => Promise<void>) | null = null

  constructor(
    readonly label: string,
    private readonly config: VoiceAgentConfig,
    private readonly bargeIn: BargeInSetting,
  ) {}

  clock(): number {
    return Date.now() - this.t0
  }

  async setup(): Promise<void> {
    const { agentOut, agentInbound, userInbound, userOut, cleanup } =
      await createBidirectionalLoopback()
    this.cleanupLoopback = cleanup
    this.echo = new VoiceAgent({
      stt: this.config.stt,
      tts: this.config.tts,
      events: { mode: 'stream' },
      vad: echoVadConfig(this.config, this.bargeIn),
    })
    await this.echo.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
    await this.echo.start()

    this.t0 = Date.now()
    void this.pump()
    this.metering = true
    void this.meter(userInbound)

    await streamSilence(userOut, WARMUP_S)
    this.writer = new PacedWriter(userOut)
    this.writer.reanchor()
  }

  async teardown(): Promise<void> {
    this.metering = false
    await this.echo?.stop().catch(() => undefined)
    await this.cleanupLoopback?.().catch(() => undefined)
  }

  private async pump(): Promise<void> {
    try {
      for await (const raw of this.echo.speechEvents()) {
        logRoundtripSpeechEvent(`${this.label} echo`, raw)
        const event: SlowFinalEvent = {
          type: raw.type,
          atMs: this.clock(),
          utteranceId: raw.utteranceId,
          text: raw.text,
        }
        this.events.push(event)
        this.onEvent(event, raw)
      }
    } catch (error) {
      console.error(`[${this.label}] speech event stream error:`, error)
    }
  }

  /** Audio the speaker receives from the echo agent, with its RMS. */
  private async meter(track: RemoteAudioTrack): Promise<void> {
    while (this.metering) {
      try {
        const pcm = await track.readSample()
        if (pcm.length === 0) {
          await sleep(10)
          continue
        }
        this.frames.push({ atMs: this.clock(), rms: pcmRms(pcm) })
      } catch {
        await sleep(20)
      }
    }
  }

  /** Send a rendered phrase, paced; records the send time of its first voiced frame. */
  async send(key: PhraseKey, frames: CapturedFrame[], word?: string): Promise<PhraseRef> {
    let sentAtMs = -1
    for (let i = 0; i < frames.length; i++) {
      const wallAt = await this.writer.write(frames[i]!.data, frames[i]!.durationMs)
      if (i === 0) sentAtMs = wallAt - this.t0
    }
    const ref: PhraseRef = { label: key, sentAtMs, word }
    this.sends.push(ref)
    console.log(
      `[${this.label}] speaker sent ${key} "${PHRASES[key]}" first frame at +${sentAtMs} ms`,
    )
    return ref
  }

  count(type: string): number {
    return this.events.filter((e) => e.type === type).length
  }

  first(type: string): SlowFinalEvent | undefined {
    return this.events.find((e) => e.type === type)
  }
}

/** Phase result: failure lines tagged with the phase, plus the timeline when anything failed. */
function collect(h: PhaseHarness, checks: Array<[string, CheckResult]>): string[] {
  const failures: string[] = []
  for (const [name, result] of checks) {
    if (result.ok) {
      console.log(`[slow-final] ${h.label} ${name}: PASS`)
    } else {
      console.log(`[slow-final] ${h.label} ${name}: FAIL ${result.reason}`)
      failures.push(`${h.label} ${name}: ${result.reason}`)
    }
  }
  if (failures.length > 0) {
    failures.push(`${h.label} echo timeline:`)
    failures.push(...formatTimeline(h.events, h.sends).map((l) => `  ${l}`))
  }
  return failures
}

/** Run `body` on a harness; a thrown error exits with the phase tag and the timeline so far. */
async function runPhase(h: PhaseHarness, body: () => Promise<void>): Promise<void> {
  try {
    await h.setup()
    await body()
  } catch (error) {
    await h.teardown()
    exitSherpaRoundtripFailure({
      reason: `${h.label} aborted`,
      failures: [
        `${h.label}: ${error instanceof Error ? error.message : String(error)}`,
        `${h.label} echo timeline:`,
        ...formatTimeline(h.events, h.sends).map((l) => `  ${l}`),
      ],
      error,
    })
  }
}

function speak(h: PhaseHarness, text: string): void {
  void h.echo.sendTextToTTS(text).catch((error: unknown) => {
    console.error(`[${h.label}] echo sendTextToTTS failed:`, error)
  })
}

/** Phase A: P2 follows P1 while P1's final is pending; the echo answers each final. */
async function phaseA(
  pcm: Record<PhraseKey, CapturedFrame[]>,
  cfg: VoiceAgentConfig,
): Promise<string[]> {
  const h = new PhaseHarness('phase A', cfg, { enabled: false })
  h.onEvent = (event) => {
    if (event.type === SPEECH_EVENT_TYPE.userSpeechFinal && (event.text ?? '').trim() !== '') {
      speak(h, event.text!.trim())
    }
  }
  let p1!: PhraseRef
  let p2!: PhraseRef
  await runPhase(h, async () => {
    await h.writer.silence(LEAD_IN_MS)
    p1 = await h.send('P1', pcm.P1, 'three')
    await h.writer.silence(P2_GAP_MS)
    p2 = await h.send('P2', pcm.P2, 'six')
    await h.writer.silenceUntil(
      () =>
        h.count(SPEECH_EVENT_TYPE.userSpeechFinal) >= 2 &&
        h.count(SPEECH_EVENT_TYPE.agentSpeakingStart) >= 2,
      PHASE_WAIT_MAX_MS,
      'both finals and both echo replies',
    )
    await h.writer.silence(LEAD_IN_MS)
  })
  await h.teardown()
  return collect(h, [
    ['next turn on time', checkNextTurnOnTime({ events: h.events, next: p2 })],
    ['final order', checkFinalOrder({ events: h.events, first: p1, second: p2 })],
    [
      'overlaps the pending final',
      checkOverlapsPendingFinal(
        h.events,
        SPEECH_EVENT_TYPE.userSpeakingStart,
        utteranceIdOf(h.events, p2),
        utteranceIdOf(h.events, p1),
      ),
    ],
    ['no user_stt_not_found', checkNoSttNotFound(h.events)],
    ['two replies', checkRepliesStarted({ events: h.events, expected: 2 })],
  ])
}

/**
 * Phases B and C share a script. The echo host starts the filler on the speaker's clock,
 * FILLER_AFTER_PHRASE_END_MS after the earlier phrase's last frame (not on `user_speaking_end`,
 * which is paired with the delayed final), and the later phrase is sent 1 s after the filler's
 * `agent_speaking_start`.
 */
async function bargePhase(params: {
  label: string
  cfg: VoiceAgentConfig
  bargeIn: BargeInSetting
  earlier: PhraseKey
  later: PhraseKey
  laterWord?: string
  pcm: Record<PhraseKey, CapturedFrame[]>
}): Promise<{ h: PhaseHarness; earlier: PhraseRef; later: PhraseRef }> {
  const h = new PhaseHarness(params.label, params.cfg, params.bargeIn)
  let earlier!: PhraseRef
  let later!: PhraseRef
  await runPhase(h, async () => {
    await h.writer.silence(LEAD_IN_MS)
    earlier = await h.send(params.earlier, params.pcm[params.earlier])
    const earlierEndedAtMs = h.clock()
    await h.writer.silenceUntil(
      () => h.clock() >= earlierEndedAtMs + FILLER_AFTER_PHRASE_END_MS,
      PHASE_WAIT_MAX_MS,
      'the filler start point',
    )
    speak(h, FILLER_TEXT)
    await h.writer.silenceUntil(
      () => h.first(SPEECH_EVENT_TYPE.agentSpeakingStart) != null,
      PHASE_WAIT_MAX_MS,
      'the echo filler (agent_speaking_start)',
    )
    const fillerStartAtMs = h.first(SPEECH_EVENT_TYPE.agentSpeakingStart)!.atMs
    await h.writer.silenceUntil(
      () => h.clock() >= fillerStartAtMs + BARGE_AFTER_FILLER_START_MS,
      PHASE_WAIT_MAX_MS,
      'the interrupt point',
    )
    later = await h.send(params.later, params.pcm[params.later], params.laterWord)
    await h.writer.silenceUntil(
      () => h.count(SPEECH_EVENT_TYPE.userSpeechFinal) >= 2,
      PHASE_WAIT_MAX_MS,
      'both finals',
    )
    await h.writer.silence(LEAD_IN_MS)
  })
  await h.teardown()
  return { h, earlier, later }
}

async function phaseB(
  pcm: Record<PhraseKey, CapturedFrame[]>,
  cfg: VoiceAgentConfig,
): Promise<string[]> {
  const { h, earlier, later } = await bargePhase({
    label: 'phase B',
    cfg,
    bargeIn: {
      enabled: true,
      useVad: true,
      requireSttPartial: false,
      flushTts: true,
      agentPlaybackGuardMs: 0,
    },
    earlier: 'P3',
    later: 'P4',
    pcm,
  })
  const agentStart = h.first(SPEECH_EVENT_TYPE.agentSpeakingStart)
  const barge = h.first(SPEECH_EVENT_TYPE.bargeIn)
  const audio: CheckResult =
    agentStart != null && barge != null
      ? checkAudioSilencedAfterBarge({
          frames: h.frames,
          agentStartAtMs: agentStart.atMs,
          bargeAtMs: barge.atMs,
        })
      : { ok: false, reason: 'no agent_speaking_start or barge_in to anchor the audio window' }
  return collect(h, [
    ['VAD barge-in on time', checkVadBargeOnTime({ events: h.events, interrupt: later })],
    ['agent audio stops', audio],
    ['final order', checkFinalOrder({ events: h.events, first: earlier, second: later })],
    [
      'overlaps the pending final',
      checkOverlapsPendingFinal(
        h.events,
        SPEECH_EVENT_TYPE.bargeIn,
        undefined,
        utteranceIdOf(h.events, earlier),
      ),
    ],
  ])
}

async function phaseC(
  pcm: Record<PhraseKey, CapturedFrame[]>,
  cfg: VoiceAgentConfig,
): Promise<string[]> {
  const { h, earlier, later } = await bargePhase({
    label: 'phase C',
    cfg,
    bargeIn: {
      enabled: true,
      useVad: true,
      requireSttPartial: true,
      flushTts: true,
      agentPlaybackGuardMs: 0,
    },
    earlier: 'P5',
    later: 'P6',
    laterWord: 'six',
    pcm,
  })
  return collect(h, [
    [
      'semantic barge-in after the earlier final',
      checkSemanticBargeAfterEarlierFinal({ events: h.events, earlier, later }),
    ],
    [
      'overlaps the pending final',
      checkOverlapsPendingFinal(
        h.events,
        SPEECH_EVENT_TYPE.userSpeakingStart,
        utteranceIdOf(h.events, later),
        utteranceIdOf(h.events, earlier),
      ),
    ],
  ])
}

async function main(): Promise<void> {
  installRoundtripWallClockTimeout(150_000)

  // Test-only knob: every final of the echo agent's STT is delayed by FINAL_DELAY_MS. Process-wide
  // (the speaker has no STT), set before any VoiceAgent exists so the native layer sees it.
  process.env.SPEECH_STT_TEST_FINAL_DELAY_MS = String(FINAL_DELAY_MS)

  const { config: cfg, label, sttModelPath, ttsModelPath } = resolveRoundtripVoiceConfig()
  if (cfg.tts == null) throw new Error('resolveRoundtripVoiceConfig returned no tts config')

  console.log('=== Sherpa slow-final roundtrip ===')
  console.log(`Pipeline: ${label}`)
  console.log(`SPEECH_STT_TEST_FINAL_DELAY_MS=${FINAL_DELAY_MS}`)
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log('')

  const pcm = await renderPhrases(cfg.tts, PHRASES)

  const failures: string[] = []
  failures.push(...(await phaseA(pcm, cfg)))
  failures.push(...(await phaseB(pcm, cfg)))
  failures.push(...(await phaseC(pcm, cfg)))

  if (failures.length > 0) {
    exitSherpaRoundtripFailure({ reason: 'slow-final assertions failed', failures })
  }

  console.log(
    '\nSlow-final roundtrip OK — next turn and barge-in were on time with a final pending.',
  )
  process.exit(0)
}

const isMain = process.argv[1]?.endsWith('roundtrip-slow-final.ts') === true

if (isMain) {
  main().catch((error: unknown) => {
    exitSherpaRoundtripFailure({
      reason: 'uncaught error',
      error,
    })
  })
}
