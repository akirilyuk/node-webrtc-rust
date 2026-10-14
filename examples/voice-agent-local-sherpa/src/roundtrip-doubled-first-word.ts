/**
 * Sherpa smoke — doubled first word in the STT final ("okay okay seven three two six ten").
 *
 * Symptom (staging load tests, 45 conversations): the agent replies "Okay. <4-5 number words>" and
 * in ~2 % of turns the client VoiceAgent (local Sherpa STT) reports ONE `user_speech_final` that
 * starts with "okay okay", although the reply audio contains "okay" once.
 *
 * What this script does:
 *   1. Generates N seeded phrases ("Okay. three, two four eight one", ...).
 *   2. Renders each with Sherpa TTS (Piper amy-medium, speed 0.8) through a speaker VoiceAgent on a
 *      throwaway loopback and captures the decoded 48 kHz stereo PCM the far peer receives
 *      (cached under `.test-logs/doubled-word/render/`).
 *   3. Feeds every phrase, followed by 6 s of silence, into a listener VoiceAgent with the client
 *      config, paced 20 ms per frame from an anchor, and collects every `user_speech_final`.
 *   4. Prints one line per phrase and exits 1 when any final starts with "okay okay".
 *
 * Variants (`DOUBLED_WORD_VARIANT`):
 *   client  the load-test client config (VOICE_AGENT_VAD_PRESET + energy 0.05, gateStt true)
 *   nogate  same, `gateStt: false` (audio streams to STT continuously, no pre-roll splice)
 *   fast    client config, no loopback and no pacing: frames go to `processInboundPcm` back-to-back
 *
 * Replay (`DOUBLED_WORD_REPLAY=<path.pcm>`): feeds one saved PCM (6 s silence before and after)
 * `DOUBLED_WORD_REPLAYS` times (default 20) through the chosen variant and counts doubled finals.
 * `DOUBLED_WORD_REPLAY_FRESH=1` creates a new VoiceAgent for every replay.
 *
 * Every doubled phrase is saved as `.test-logs/doubled-word/<index>.pcm` (client variant) or
 * `<variant>-<index>.pcm` (48 kHz stereo s16le) with a `.txt` sidecar holding phrase and finals.
 *
 * Env: DOUBLED_WORD_PHRASES (default 120), DOUBLED_WORD_SEED, DOUBLED_WORD_TTS_SPEED (default 0.8),
 * SHERPA_STT_MODEL_PATH, SHERPA_TTS_MODEL_PATH, VOICE_DEBUG=1 for Rust `[voice-debug]` lines.
 *
 * Run:
 *   npm run start:roundtrip-doubled-first-word --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 */

import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import type { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import {
  SPEECH_EVENT_TYPE,
  VOICE_AGENT_VAD_PRESET,
  VoiceAgent,
  type SpeechEvent,
  type TtsConfig,
  type VoiceAgentConfig,
} from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { PCM_FRAME_DURATION_MS, PCM_FULL_FRAME_BYTES } from '../../shared/pcm-streaming.js'
import { stereoPcmDurationMs, streamSilence } from './pcm-relay.js'
import {
  attributeFinals,
  DEFAULT_PHRASE_SEED,
  FINAL_TAIL_MS,
  generatePhrases,
  isDoubledFirstWord,
  parsePhraseCount,
  parseVariant,
  SILENCE_BETWEEN_PHRASES_MS,
  variantGateStt,
  variantIsPaced,
  wallClockBudgetMs,
  type DoubledWordVariant,
  type PhraseWindow,
  type TimedFinal,
} from './roundtrip-doubled-first-word-helpers.js'
import {
  AgentSpeakingEndLatch,
  installRoundtripWallClockTimeout,
  startSpeakerSpeechPump,
} from './roundtrip-counting.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import {
  enableRoundtripSpeechEventLog,
  logRoundtripSpeechEvent,
} from './roundtrip-speech-events.js'

const WARMUP_S = 0.6
// `<repo>/.test-logs/doubled-word`, independent of the npm workspace cwd.
const OUT_DIR = fileURLToPath(new URL('../../../.test-logs/doubled-word', import.meta.url))
const RENDER_DIR = join(OUT_DIR, 'render')
const AUDIBLE_PEAK = 150
const TTS_RENDER_TIMEOUT_MS = 40_000
const DEFAULT_REPLAYS = 20

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function requireEnv(name: string): string {
  const value = process.env[name]?.trim()
  if (!value)
    throw new Error(`${name} is not set (see examples/voice-agent-local-sherpa/README.md)`)
  return value
}

function peak(data: Buffer): number {
  let max = 0
  for (let i = 0; i + 1 < data.length; i += 2) max = Math.max(max, Math.abs(data.readInt16LE(i)))
  return max
}

/** Split interleaved stereo s16le PCM into 20 ms frames (the last one zero-padded). */
function toFrames(pcm: Buffer): Buffer[] {
  const frames: Buffer[] = []
  for (let off = 0; off < pcm.length; off += PCM_FULL_FRAME_BYTES) {
    const frame = Buffer.alloc(PCM_FULL_FRAME_BYTES)
    pcm.copy(frame, 0, off, Math.min(off + PCM_FULL_FRAME_BYTES, pcm.length))
    frames.push(frame)
  }
  return frames
}

/** Drop idle frames before the first and after the last audible frame (keep one frame of pad). */
function trimSilence(frames: Buffer[]): Buffer[] {
  const first = frames.findIndex((f) => peak(f) > AUDIBLE_PEAK)
  if (first < 0) return []
  let last = frames.length - 1
  while (last > first && peak(frames[last]!) <= AUDIBLE_PEAK) last -= 1
  return frames.slice(Math.max(0, first - 1), last + 2)
}

function cachePath(phrase: string, ttsPath: string, speed: string): string {
  const key = createHash('sha1').update(`${phrase}|${ttsPath}|${speed}`).digest('hex').slice(0, 16)
  return join(RENDER_DIR, `${key}.pcm`)
}

function ttsConfigFor(ttsPath: string, speed: string): TtsConfig {
  // `TtsConfig.model` carries the Sherpa speed ("0.8"), see `parse_speed` in vendor-sherpa-onnx.
  return { provider: 'local-sherpa', modelPath: ttsPath, voice: '0', model: speed }
}

/**
 * Render every phrase that is not cached yet with a speaker VoiceAgent and capture the decoded PCM
 * the far peer receives (TTS PCM is written natively, not through `writeSample`).
 */
async function renderPhrases(
  phrases: string[],
  ttsPath: string,
  speed: string,
): Promise<Map<string, Buffer>> {
  mkdirSync(RENDER_DIR, { recursive: true })
  const rendered = new Map<string, Buffer>()
  const missing: string[] = []
  for (const phrase of phrases) {
    const file = cachePath(phrase, ttsPath, speed)
    if (existsSync(file)) rendered.set(phrase, readFileSync(file))
    else if (!missing.includes(phrase)) missing.push(phrase)
  }
  console.log(
    `[doubled-smoke] TTS render: ${rendered.size} cached, ${missing.length} to synthesize (speed ${speed})`,
  )
  if (missing.length === 0) return rendered

  const { agentOut, agentInbound, userInbound, cleanup } = await createBidirectionalLoopback()
  const speaker = new VoiceAgent({
    tts: ttsConfigFor(ttsPath, speed),
    events: { mode: 'stream' },
    vad: { enabled: false },
  })
  await speaker.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await speaker.start()
  await streamSilence(agentOut, WARMUP_S)

  const latch = new AgentSpeakingEndLatch()
  startSpeakerSpeechPump(speaker, latch)

  const capture: { chunks: Buffer[] | null } = { chunks: null }
  let capturing = true
  void (async () => {
    while (capturing) {
      try {
        const pcm = await userInbound.readSample()
        if (pcm.length === 0) {
          await sleep(5)
          continue
        }
        capture.chunks?.push(Buffer.from(pcm))
      } catch {
        await sleep(20)
      }
    }
  })()

  for (const [i, phrase] of missing.entries()) {
    capture.chunks = []
    const baseline = latch.endEventsSeen()
    await speaker.sendTextToTTS(phrase)
    await latch.waitAfterCount(baseline, TTS_RENDER_TIMEOUT_MS)
    await sleep(300)
    const captured = capture.chunks
    capture.chunks = null
    const frames = trimSilence(toFrames(Buffer.concat(captured)))
    if (frames.length === 0) throw new Error(`TTS rendered no audible audio for "${phrase}"`)
    const pcm = Buffer.concat(frames)
    writeFileSync(cachePath(phrase, ttsPath, speed), pcm)
    rendered.set(phrase, pcm)
    if ((i + 1) % 10 === 0 || i + 1 === missing.length) {
      console.log(`[doubled-smoke] rendered ${i + 1}/${missing.length}`)
    }
  }

  capturing = false
  await speaker.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)
  return rendered
}

/** Writes PCM paced from an anchor: frame n is due at `anchor + n * 20 ms`; never bursts to catch up. */
class PacedWriter {
  private anchorAt = performance.now()
  private sentMs = 0

  constructor(private readonly track: LocalAudioTrack) {}

  reanchor(): void {
    this.anchorAt = performance.now()
    this.sentMs = 0
  }

  async write(frame: Buffer): Promise<void> {
    const durationMs = stereoPcmDurationMs(frame.length)
    const waitMs = this.anchorAt + this.sentMs - performance.now()
    if (waitMs > 0) await sleep(waitMs)
    else if (waitMs < -100) this.reanchor()
    await this.track.writeSample(frame, durationMs)
    this.sentMs += durationMs
  }
}

/** The client VoiceAgent config of the load test (variant `nogate` only flips `gateStt`). */
function clientConfig(variant: DoubledWordVariant, sttPath: string): VoiceAgentConfig {
  return {
    stt: { provider: 'local-sherpa', language: 'en', modelPath: sttPath },
    events: { mode: 'stream' },
    vad: {
      ...VOICE_AGENT_VAD_PRESET,
      provider: 'energy',
      threshold: 0.05,
      minSpeechDurationMs: 200,
      sttGateHoldMs: 1000,
      gateStt: variantGateStt(variant),
      bargeIn: { ...VOICE_AGENT_VAD_PRESET.bargeIn, enabled: false },
    },
  }
}

interface RecordedEvent {
  type: string
  atMs: number
  text?: string
}

/** A listener VoiceAgent on a loopback (or without traffic for `fast`) that records its events. */
class ListenerSession {
  readonly events: RecordedEvent[] = []
  private readonly t0 = performance.now()
  private writer: PacedWriter | null = null
  private cleanup: (() => Promise<void>) | null = null
  private listener: VoiceAgent | null = null

  constructor(
    private readonly variant: DoubledWordVariant,
    private readonly sttPath: string,
  ) {}

  nowMs(): number {
    return Math.round(performance.now() - this.t0)
  }

  async open(): Promise<void> {
    const { agentOut, userInbound, userOut, cleanup } = await createBidirectionalLoopback()
    this.cleanup = cleanup
    const listener = new VoiceAgent(clientConfig(this.variant, this.sttPath))
    await listener.attach({ inboundTrack: userInbound, outboundTrack: userOut })
    await listener.start()
    this.listener = listener
    void (async () => {
      for await (const event of listener.speechEvents()) this.record(event)
    })()
    await streamSilence(agentOut, WARMUP_S)
    this.writer = new PacedWriter(agentOut)
    this.writer.reanchor()
  }

  private record(event: SpeechEvent): void {
    logRoundtripSpeechEvent('listener', event)
    this.events.push({ type: event.type, atMs: this.nowMs(), text: event.text })
  }

  finals(): TimedFinal[] {
    return this.events
      .filter((e) => e.type === SPEECH_EVENT_TYPE.userSpeechFinal)
      .map((e) => ({ atMs: e.atMs, text: (e.text ?? '').trim() }))
  }

  /** Feed frames: paced through the loopback, or back-to-back into `processInboundPcm` for `fast`. */
  async feed(frames: Buffer[]): Promise<void> {
    if (variantIsPaced(this.variant)) {
      for (const frame of frames) await this.writer!.write(frame)
      return
    }
    const native = this.listener!.getNativeAgent()
    for (const frame of frames) await native.processInboundPcm(frame, PCM_FRAME_DURATION_MS)
  }

  async feedSilence(ms: number): Promise<void> {
    const silent = Buffer.alloc(PCM_FULL_FRAME_BYTES)
    const count = Math.ceil(ms / PCM_FRAME_DURATION_MS)
    await this.feed(Array.from({ length: count }, () => silent))
  }

  /** After a pause without writes, restart the 20 ms schedule from now. */
  reanchor(): void {
    this.writer?.reanchor()
  }

  async close(): Promise<void> {
    await this.listener?.stop().catch(() => undefined)
    await this.cleanup?.().catch(() => undefined)
  }
}

function savePhrase(
  variant: DoubledWordVariant,
  index: number,
  phrase: string,
  pcm: Buffer,
  finals: TimedFinal[],
): void {
  mkdirSync(OUT_DIR, { recursive: true })
  const stem = join(OUT_DIR, variant === 'client' ? `${index}` : `${variant}-${index}`)
  writeFileSync(`${stem}.pcm`, pcm)
  writeFileSync(
    `${stem}.txt`,
    [
      `variant: ${variant}`,
      `index: ${index}`,
      `phrase: ${phrase}`,
      ...finals.map((f) => `final@${f.atMs}ms: ${f.text}`),
      '',
    ].join('\n'),
  )
}

async function runSmoke(
  variant: DoubledWordVariant,
  sttPath: string,
  ttsPath: string,
): Promise<void> {
  const count = parsePhraseCount(process.env.DOUBLED_WORD_PHRASES)
  const seed = Number(process.env.DOUBLED_WORD_SEED ?? DEFAULT_PHRASE_SEED)
  const speed = process.env.DOUBLED_WORD_TTS_SPEED?.trim() || '0.8'
  const phrases = generatePhrases(count, seed)

  const rendered = await renderPhrases(phrases, ttsPath, speed)

  const session = new ListenerSession(variant, sttPath)
  await session.open()
  const windows: PhraseWindow[] = []
  try {
    await session.feedSilence(3000)
    for (const [index, phrase] of phrases.entries()) {
      const pcm = rendered.get(phrase)!
      const startMs = session.nowMs()
      console.log(`[doubled-smoke] phrase #${index} start@${startMs}ms "${phrase}"`)
      await session.feed(toFrames(pcm))
      await session.feedSilence(SILENCE_BETWEEN_PHRASES_MS)
      windows.push({ index, startMs, endMs: session.nowMs() })
      // Unpaced frames return long before the pipeline has produced its final.
      if (!variantIsPaced(variant)) await sleep(FINAL_TAIL_MS)
      session.reanchor()
    }
    await sleep(FINAL_TAIL_MS)
  } catch (error) {
    await session.close()
    exitSherpaRoundtripFailure({ reason: 'doubled first word smoke aborted', error })
  }

  const byPhrase = attributeFinals(session.finals(), windows)
  const doubledIndexes: number[] = []
  let silent = 0
  for (const [index, phrase] of phrases.entries()) {
    const finals = byPhrase.get(index) ?? []
    const doubled = finals.some((f) => isDoubledFirstWord(f.text))
    if (finals.length === 0) silent += 1
    if (doubled) {
      doubledIndexes.push(index)
      savePhrase(variant, index, phrase, rendered.get(phrase)!, finals)
    }
    console.log(
      `[doubled-smoke] #${index} ref="${phrase}" finals=${JSON.stringify(finals.map((f) => f.text))}${doubled ? ' DOUBLED' : ''}`,
    )
  }
  await session.close()

  console.log(
    `[doubled-smoke] variant=${variant} phrases=${count} no_final=${silent} doubled=${doubledIndexes.length}/${count}`,
  )
  if (doubledIndexes.length > 0) {
    exitSherpaRoundtripFailure({
      reason: `doubled first word in ${doubledIndexes.length}/${count} finals (variant ${variant})`,
      failures: doubledIndexes.map((i) => `#${i} "${phrases[i]}"`),
    })
  }
  console.log('Doubled first word smoke OK — no final started with "okay okay".')
  process.exit(0)
}

async function runReplay(
  variant: DoubledWordVariant,
  sttPath: string,
  replayPath: string,
): Promise<void> {
  const pcmFrames = toFrames(readFileSync(replayPath))
  const replays = Number(process.env.DOUBLED_WORD_REPLAYS ?? DEFAULT_REPLAYS)
  const fresh = process.env.DOUBLED_WORD_REPLAY_FRESH === '1'
  console.log(
    `[doubled-smoke] replay file=${replayPath} frames=${pcmFrames.length} variant=${variant} replays=${replays} fresh=${fresh}`,
  )

  let session: ListenerSession | null = null
  let doubled = 0
  for (let n = 0; n < replays; n++) {
    if (session === null || fresh) {
      await session?.close()
      session = new ListenerSession(variant, sttPath)
      await session.open()
    }
    const windowStart = session.nowMs()
    console.log(`[doubled-smoke] replay #${n} start@${windowStart}ms`)
    try {
      await session.feedSilence(SILENCE_BETWEEN_PHRASES_MS)
      await session.feed(pcmFrames)
      await session.feedSilence(SILENCE_BETWEEN_PHRASES_MS)
      if (!variantIsPaced(variant)) await sleep(FINAL_TAIL_MS)
    } catch (error) {
      await session.close()
      exitSherpaRoundtripFailure({ reason: 'doubled first word replay aborted', error })
    }
    const finals = session
      .finals()
      .filter((f) => f.atMs >= windowStart)
      .map((f) => f.text)
    const isDoubled = finals.some(isDoubledFirstWord)
    if (isDoubled) doubled += 1
    console.log(
      `[doubled-smoke] replay #${n} finals=${JSON.stringify(finals)}${isDoubled ? ' DOUBLED' : ''}`,
    )
    session.reanchor()
  }
  await session?.close()

  console.log(`[doubled-smoke] replay variant=${variant} doubled=${doubled}/${replays}`)
  if (doubled > 0) {
    exitSherpaRoundtripFailure({
      reason: `replay doubled ${doubled}/${replays} (variant ${variant})`,
      failures: [replayPath],
    })
  }
  process.exit(0)
}

async function main(): Promise<void> {
  enableRoundtripSpeechEventLog()
  const variant = parseVariant(process.env.DOUBLED_WORD_VARIANT)
  const count = parsePhraseCount(process.env.DOUBLED_WORD_PHRASES)
  const replayPath = process.env.DOUBLED_WORD_REPLAY?.trim()
  const wallMs =
    replayPath != null && replayPath !== ''
      ? 300_000 +
        Number(process.env.DOUBLED_WORD_REPLAYS ?? DEFAULT_REPLAYS) *
          (2 * SILENCE_BETWEEN_PHRASES_MS + 12_000 + FINAL_TAIL_MS)
      : wallClockBudgetMs(count, variant)
  installRoundtripWallClockTimeout(wallMs)

  const sttPath = requireEnv('SHERPA_STT_MODEL_PATH')
  const ttsPath = requireEnv('SHERPA_TTS_MODEL_PATH')

  console.log('=== Sherpa doubled first word smoke ===')
  console.log(`variant=${variant} replay=${replayPath ?? '(none)'}`)
  console.log(`SHERPA_STT_MODEL_PATH=${sttPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsPath}`)
  console.log(`vad=${JSON.stringify(clientConfig(variant, sttPath).vad)}`)

  if (replayPath != null && replayPath !== '') {
    await runReplay(variant, sttPath, replayPath)
    return
  }
  await runSmoke(variant, sttPath, ttsPath)
}

const isMain = process.argv[1]?.endsWith('roundtrip-doubled-first-word.ts') === true

if (isMain) {
  main().catch((error: unknown) => {
    exitSherpaRoundtripFailure({ reason: 'uncaught error', error })
  })
}
