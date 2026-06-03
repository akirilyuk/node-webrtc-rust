/**
 * Sherpa roundtrip — **one long utterance** (count 1–20) must produce a **single** STT final.
 *
 * Regression for VAD gate-hold / mid-utterance `user_speaking_end` splits when the speaker
 * pauses briefly between words (see browser multi-client counting tests).
 *
 *   Speaker: Sherpa TTS plays one long phrase on agentOut
 *   Listener: VAD + gateStt + Sherpa STT (production preset from resolveVoiceConfig)
 *
 * Shared harness (also used by `roundtrip.ts`, echo, two-phrases, utterance-timing):
 *   `AgentSpeakingEndLatch`, `startSpeakerSpeechPump`, `playSpeakerTtsWithPostSilence` —
 *   see ROUNDTRIP.md § "Harness playback timing".
 *
 * Run (requires models — same as roundtrip):
 *   npm run start:roundtrip-counting --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Env:
 *   SHERPA_COUNTING_PHRASE              override TTS text (default: "one" … "twenty")
 *   SHERPA_COUNTING_TIMEOUT_MS          wait for transcript (default 90000)
 *   SHERPA_COUNTING_MIN_NUMBER_WORDS    min of 20 number words in final (default 16)
 *   SHERPA_COUNTING_VERBOSE             set to 1 for per-event logs
 */

import type { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import { VoiceAgent, VOICE_AGENT_VAD_PRESET, SPEECH_EVENT_TYPE } from '@node-webrtc-rust/sdk/voice'
import type { SpeechEvent, SpeechEventType, VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import {
  currentRoundtripScript,
  enableSherpaRoundtripRustDebug,
  exitSherpaRoundtripFailure,
  rememberRoundtripEntryScript,
} from './roundtrip-failure-debug.js'
import { logRoundtripSpeechEvent } from './roundtrip-speech-events.js'
import { streamSilence } from './pcm-relay.js'
import { resolveVoiceConfig } from './resolve-voice-config.js'
import { evaluateNormalUtteranceLifecycle } from './roundtrip-stt-lifecycle-helpers.js'

/** Default long utterance: spoken digits one through twenty (no commas — natural counting). */
export const DEFAULT_COUNTING_PHRASE =
  'one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty'

/** Shorter phrase for bidirectional echo roundtrip (one … ten). */
export const DEFAULT_COUNTING_PHRASE_ONE_TO_TEN = 'one two three four five six seven eight nine ten'

export const NUMBER_WORDS_ONE_TO_TWENTY = [
  'one',
  'two',
  'three',
  'four',
  'five',
  'six',
  'seven',
  'eight',
  'nine',
  'ten',
  'eleven',
  'twelve',
  'thirteen',
  'fourteen',
  'fifteen',
  'sixteen',
  'seventeen',
  'eighteen',
  'nineteen',
  'twenty',
] as const

export const NUMBER_WORDS_ONE_TO_TEN = NUMBER_WORDS_ONE_TO_TWENTY.slice(0, 10)

/** STT / final wait (gate hold + Sherpa finalize). Override with SHERPA_COUNTING_TIMEOUT_MS. */
const DEFAULT_TIMEOUT_MS = 45_000
const DEFAULT_MIN_NUMBER_WORDS = 16
/** Harness scheduling jitter — not part of Rust VAD/STT close. */
export const FINALIZE_MARGIN_MS = 250
const DEFAULT_WARMUP_S = 0.6
/** Outbound TTS drain cap — wall-clock estimate, always bounded. */
export const DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS = 45_000

/**
 * Hard process kill for E2E scripts — overrides hung STT/native waits so CI/local runs fail fast.
 * Override with `SHERPA_ROUNDTRIP_WALL_MS` (applies to all roundtrip `start:*` scripts).
 */
export function installRoundtripWallClockTimeout(defaultWallMs = 90_000): void {
  rememberRoundtripEntryScript()
  const scriptName = currentRoundtripScript()
  enableSherpaRoundtripRustDebug()
  const rawWall = process.env.SHERPA_ROUNDTRIP_WALL_MS
  let wallMs = rawWall != null && rawWall !== '' ? Number(rawWall) : defaultWallMs
  if (!Number.isFinite(wallMs) || wallMs <= 0) {
    console.warn(
      `[${scriptName}] invalid SHERPA_ROUNDTRIP_WALL_MS=${String(rawWall)} — using default ${defaultWallMs} ms`,
    )
    wallMs = defaultWallMs
  }
  console.log(`[${scriptName}] wall-clock limit ${wallMs} ms (SHERPA_ROUNDTRIP_WALL_MS)`)
  setTimeout(() => {
    exitSherpaRoundtripFailure({
      reason: `wall-clock timeout after ${wallMs} ms (SHERPA_ROUNDTRIP_WALL_MS)`,
    })
  }, wallMs)
}

function sleepMs(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/**
 * One-shot wait for `agent_speaking_end` on the **speaker** VoiceAgent stream.
 * Wire via `startSpeakerSpeechPump` — the listener leg does not receive agent events.
 */
export class AgentSpeakingEndLatch {
  private endCount = 0
  private waiters: Array<{ baseline: number; resolve: () => void }> = []

  /** Count of `agent_speaking_end` observed on the speaker stream so far. */
  endEventsSeen(): number {
    return this.endCount
  }

  observe(event: SpeechEvent): void {
    if (event.type !== SPEECH_EVENT_TYPE.agentSpeakingEnd) return
    this.endCount += 1
    for (let i = this.waiters.length - 1; i >= 0; i--) {
      const waiter = this.waiters[i]!
      if (this.endCount > waiter.baseline) {
        waiter.resolve()
        this.waiters.splice(i, 1)
      }
    }
  }

  /** Resolve when `agent_speaking_end` count exceeds `baseline` (capture baseline before `sendTextToTTS`). */
  waitAfterCount(baseline: number, timeoutMs: number): Promise<void> {
    if (this.endCount > baseline) {
      return Promise.resolve()
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiters = this.waiters.filter((w) => w !== waiter)
        reject(new Error(`timed out waiting for agent_speaking_end (${timeoutMs} ms)`))
      }, timeoutMs)
      const waiter = {
        baseline,
        resolve: () => {
          clearTimeout(timer)
          resolve()
        },
      }
      this.waiters.push(waiter)
    })
  }

  waitForNext(timeoutMs: number): Promise<void> {
    return this.waitAfterCount(this.endCount, timeoutMs)
  }
}

/** Log speaker speech events and notify `agentEndLatch` for playback timing. */
export function startSpeakerSpeechPump(
  speaker: VoiceAgent,
  agentEndLatch?: AgentSpeakingEndLatch,
): void {
  void (async () => {
    for await (const event of speaker.speechEvents()) {
      logRoundtripSpeechEvent('speaker', event)
      agentEndLatch?.observe(event)
    }
  })()
}

/** Real-time TTS drain ~900 ms/word + 3 s base (capped). Long echo phrases need the headroom. */
export function estimateTtsPlaybackMs(phrase: string, capMs: number): number {
  const words = phrase.split(/\s+/).filter((w) => w.length > 0).length
  return Math.min(capMs, Math.max(3000, words * 900 + 3000))
}

/**
 * Wait for outbound TTS playback (wall-clock estimate).
 * Does not call native `waitTtsPlaybackIdle` — that can block the Node loop and ignore JS timeouts.
 */
export async function waitAgentTtsPlaybackEnd(
  phrase: string,
  timeoutMs = DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
): Promise<void> {
  await waitAgentPlaybackEndRace({ phrase, capMs: timeoutMs })
}

/**
 * End playback when `agent_speaking_end` arrives (via speaker-stream latch), or when the
 * phrase-length estimate elapses — whichever is first. Avoids multi-second dead air after TTS
 * already stopped (estimate was ~900 ms/word + 3 s base).
 */
export async function waitAgentPlaybackEndRace(params: {
  phrase: string
  waitForAgentSpeakingEnd?: () => Promise<void>
  capMs?: number
}): Promise<'event' | 'estimate'> {
  const capMs = params.capMs ?? DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS
  const estimateMs = estimateTtsPlaybackMs(params.phrase, capMs)
  if (!params.waitForAgentSpeakingEnd) {
    console.log(
      `[speaker] playback wait ${(estimateMs / 1000).toFixed(1)}s (estimate, cap ${(capMs / 1000).toFixed(0)}s)`,
    )
    await sleepMs(estimateMs)
    return 'estimate'
  }
  console.log(
    `[speaker] playback wait ≤${(estimateMs / 1000).toFixed(1)}s (agent_speaking_end or estimate)`,
  )
  const via = await Promise.race([
    params.waitForAgentSpeakingEnd().then(() => 'event' as const),
    sleepMs(estimateMs).then(() => 'estimate' as const),
  ])
  console.log(
    via === 'event'
      ? '[speaker] playback ended (agent_speaking_end)'
      : `[speaker] playback ended (estimate cap ${(estimateMs / 1000).toFixed(1)}s)`,
  )
  return via
}

/**
 * Play TTS on a speaker agent, wait for real-time outbound drain, then stream post-utterance silence.
 * Required for gate-hold STT: trailing silence must arrive after playback ends, not in parallel with it.
 */
export async function playSpeakerTtsWithPostSilence(params: {
  speaker: VoiceAgent
  speakerOut: LocalAudioTrack
  phrase: string
  postTtsSilenceS: number
  playbackTimeoutMs?: number
  /** Prefer speaker-stream latch from `startSpeakerSpeechPump`. */
  agentSpeakingEndLatch?: AgentSpeakingEndLatch
  /** Fallback when no speaker pump / latch (rare). */
  waitForAgentSpeakingEnd?: () => Promise<void>
}): Promise<void> {
  const playbackTimeoutMs = params.playbackTimeoutMs ?? DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS
  const playbackBaseline = params.agentSpeakingEndLatch?.endEventsSeen() ?? 0
  const waitEnd =
    params.agentSpeakingEndLatch != null
      ? () => params.agentSpeakingEndLatch!.waitAfterCount(playbackBaseline, playbackTimeoutMs)
      : params.waitForAgentSpeakingEnd
  console.log(`[speaker] TTS synthesize (${params.phrase.length} chars)…`)
  await params.speaker.sendTextToTTS(params.phrase)
  await waitAgentPlaybackEndRace({
    phrase: params.phrase,
    capMs: playbackTimeoutMs,
    waitForAgentSpeakingEnd: waitEnd,
  })
  console.log(`[speaker] post-TTS silence ${params.postTtsSilenceS.toFixed(1)}s`)
  await streamSilence(params.speakerOut, params.postTtsSilenceS)
}

export interface UtteranceEventStats {
  finals: string[]
  speakingEndCount: number
  speakingStartCount: number
  partialCount: number
  bargeInCount: number
  agentSpeakingStartCount: number
  agentSpeakingEndCount: number
  /** Wall-clock ms when the first `user_speaking_end` of the current wait was recorded. */
  speakingEndAtMs: number | null
  /** Wall-clock ms when the first `user_speech_final` of the current wait was recorded. */
  speechFinalAtMs: number | null
}

/** Timestamped speech event captured during an optional recording window on a collector. */
export interface CollectedSpeechEvent {
  type: SpeechEventType
  atMs: number
  text?: string
}

/** Default max gap between `user_speaking_end` and `user_speech_final` (regression for Sherpa close). */
export const DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS = 500

export interface SpeakingEndFinalTimingResult {
  gapMs: number | null
  passed: boolean
  failures: string[]
}

/** Assert `user_speaking_end` immediately precedes `user_speech_final` within a tight window. */
export function evaluateSpeakingEndFinalTiming(params: {
  stats: UtteranceEventStats
  maxGapMs?: number
  label?: string
}): SpeakingEndFinalTimingResult {
  const maxGapMs = params.maxGapMs ?? DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS
  const who = params.label ? `${params.label}: ` : ''
  const failures: string[] = []

  if (params.stats.speakingEndCount !== 1) {
    failures.push(
      `${who}expected exactly 1 user_speaking_end, got ${params.stats.speakingEndCount}`,
    )
  }
  if (params.stats.finals.length !== 1) {
    failures.push(`${who}expected exactly 1 user_speech_final, got ${params.stats.finals.length}`)
  }

  const endAt = params.stats.speakingEndAtMs
  const finalAt = params.stats.speechFinalAtMs
  let gapMs: number | null = null

  if (endAt == null) {
    failures.push(`${who}missing timestamp for user_speaking_end`)
  }
  if (finalAt == null) {
    failures.push(`${who}missing timestamp for user_speech_final`)
  }
  if (endAt != null && finalAt != null) {
    gapMs = finalAt - endAt
    if (gapMs < 0) {
      failures.push(
        `${who}user_speech_final arrived ${-gapMs} ms before user_speaking_end (expected end then final)`,
      )
    } else if (gapMs > maxGapMs) {
      failures.push(
        `${who}user_speaking_end → user_speech_final gap ${gapMs} ms exceeds ${maxGapMs} ms`,
      )
    }
  }

  return {
    gapMs,
    passed: failures.length === 0,
    failures,
  }
}

/** One paired `user_speaking_end` → `user_speech_final` from the event stream. */
export interface FinalEventRecord {
  text: string
  speakingEndAtMs: number
  finalAtMs: number
  gapMs: number
}

export function finalRecordFromStats(text: string, stats: UtteranceEventStats): FinalEventRecord {
  const speakingEndAtMs = stats.speakingEndAtMs ?? 0
  const finalAtMs = stats.speechFinalAtMs ?? 0
  return {
    text,
    speakingEndAtMs,
    finalAtMs,
    gapMs: finalAtMs - speakingEndAtMs,
  }
}

export interface FinalSequenceEvaluation {
  passed: boolean
  failures: string[]
}

/** Evaluate every final in a multi-phrase session (timing + optional text checks). */
export function evaluateFinalSequence(params: {
  records: FinalEventRecord[]
  expectedCount: number
  maxGapMs?: number
  label?: string
  /** Optional per-index substring checks (index 0 = first final). */
  textIncludes?: Array<string | undefined>
  minNumberWordsFirst?: number
  numberWords?: readonly string[]
}): FinalSequenceEvaluation {
  const maxGapMs = params.maxGapMs ?? DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS
  const who = params.label ? `${params.label}: ` : ''
  const failures: string[] = []
  const numberWords = params.numberWords ?? NUMBER_WORDS_ONE_TO_TEN

  if (params.records.length !== params.expectedCount) {
    failures.push(
      `${who}expected ${params.expectedCount} user_speech_final events, got ${params.records.length}`,
    )
  }

  for (let i = 0; i < params.records.length; i++) {
    const rec = params.records[i]
    if (rec.gapMs < 0) {
      failures.push(`${who}final ${i + 1}: user_speech_final before user_speaking_end`)
    } else if (rec.gapMs > maxGapMs) {
      failures.push(
        `${who}final ${i + 1}: speaking_end→final gap ${rec.gapMs} ms exceeds ${maxGapMs} ms`,
      )
    }
    const need = params.textIncludes?.[i]?.trim()
    if (need && !rec.text.toLowerCase().includes(need.toLowerCase())) {
      failures.push(`${who}final ${i + 1}: expected text to include ${JSON.stringify(need)}`)
    }
  }

  if (
    params.minNumberWordsFirst != null &&
    params.records[0] &&
    countNumberWordsInTranscript(params.records[0].text, numberWords) < params.minNumberWordsFirst
  ) {
    failures.push(
      `${who}final 1: expected at least ${params.minNumberWordsFirst} number words in "${params.records[0].text}"`,
    )
  }

  return { passed: failures.length === 0, failures }
}

/** Extra wall time between two TTS phrases so the listener VAD can end turn 1 before turn 2. */
export function interPhraseSilenceSeconds(config: VoiceAgentConfig): number {
  const extra = Number(process.env.SHERPA_TWO_PHRASE_EXTRA_GAP_S ?? 1.5)
  return postTtsSilenceSeconds(config) + extra
}

/**
 * Collects every `user_speech_final` in order (multi-turn STT — like browser multi-client).
 */
export class FinalSequenceCollector {
  readonly records: FinalEventRecord[] = []
  private pendingSpeakingEndAt: number | null = null
  private waitTarget = 0
  private waitResolve: (() => void) | null = null
  private waitReject: ((error: Error) => void) | null = null
  private waitTimer: ReturnType<typeof setTimeout> | undefined

  constructor(
    private readonly listener: VoiceAgent,
    private readonly pumpStarted: { value: boolean },
    private readonly verbose: boolean,
    private readonly agentLabel = 'listener',
  ) {}

  startPump(): void {
    if (this.pumpStarted.value) return
    this.pumpStarted.value = true
    void this.pump()
  }

  private async pump(): Promise<void> {
    try {
      for await (const event of this.listener.speechEvents()) {
        logRoundtripSpeechEvent(this.agentLabel, event)
        if (event.type === 'user_speaking_end') {
          this.pendingSpeakingEndAt = Date.now()
        }
        if (event.type === 'user_speech_final') {
          const finalAt = Date.now()
          const speakingEndAt = this.pendingSpeakingEndAt ?? finalAt
          this.pendingSpeakingEndAt = null
          const text = (event.text ?? '').trim()
          this.records.push({
            text,
            speakingEndAtMs: speakingEndAt,
            finalAtMs: finalAt,
            gapMs: finalAt - speakingEndAt,
          })
          if (this.waitResolve && this.records.length >= this.waitTarget) {
            this.clearWait()
            this.waitResolve()
          }
        }
      }
    } catch (error) {
      if (this.waitReject) {
        this.fail(error instanceof Error ? error : new Error(String(error)))
      }
    }
  }

  private clearWait(): void {
    if (this.waitTimer) clearTimeout(this.waitTimer)
    this.waitTimer = undefined
    this.waitResolve = null
    this.waitReject = null
    this.waitTarget = 0
  }

  private fail(error: Error): void {
    this.clearWait()
    this.waitReject?.(error)
  }

  /** Resolves when `records.length` reaches at least `count` (1-based count). */
  waitForFinalCount(count: number, timeoutMs: number): Promise<void> {
    if (this.records.length >= count) {
      return Promise.resolve()
    }
    if (this.waitResolve) {
      return Promise.reject(new Error('Already waiting for a final count'))
    }
    this.waitTarget = count
    return new Promise((resolve, reject) => {
      this.waitResolve = resolve
      this.waitReject = reject
      this.waitTimer = setTimeout(() => {
        console.error(
          `[listener] TIMEOUT after ${timeoutMs} ms waiting for user_speech_final #${count} (have ${this.records.length})`,
        )
        this.fail(
          new Error(
            `Timed out after ${timeoutMs} ms waiting for final #${count} (have ${this.records.length})`,
          ),
        )
      }, timeoutMs)
    })
  }
}

export interface CountingRoundtripResult {
  phrase: string
  recognized: string
  stats: UtteranceEventStats
  numberWordsFound: number
  passed: boolean
  failures: string[]
}

/** Matches Rust STT endpoint tail (`min_silence` clamped 400–600 ms). */
export function endpointTailMs(config: VoiceAgentConfig): number {
  const minSilence =
    config.vad?.minSilenceDurationMs ?? VOICE_AGENT_VAD_PRESET.minSilenceDurationMs ?? 1300
  return Math.min(600, Math.max(minSilence, 400))
}

export function sttFinalizeWaitMs(config: VoiceAgentConfig): number {
  const hold = config.vad?.sttGateHoldMs ?? VOICE_AGENT_VAD_PRESET.sttGateHoldMs ?? 1000
  return hold + endpointTailMs(config) + FINALIZE_MARGIN_MS
}

/**
 * Real-time trailing silence after TTS so listener VAD can emit SpeechEnd and gate hold can drain.
 * Rust injects the STT endpoint tail on finalize — do not add it again here.
 */
export function postTtsSilenceSeconds(config: VoiceAgentConfig): number {
  const hold = config.vad?.sttGateHoldMs ?? VOICE_AGENT_VAD_PRESET.sttGateHoldMs ?? 1000
  const minSilence =
    config.vad?.minSilenceDurationMs ?? VOICE_AGENT_VAD_PRESET.minSilenceDurationMs ?? 1300
  return (hold + minSilence + FINALIZE_MARGIN_MS) / 1000
}

export function normalizeForCompare(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9\s]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}

/** Share of `input` words (after normalize) found as whole tokens in `recognized`. */
export function wordSimilarity(input: string, recognized: string): number {
  const words = normalizeForCompare(input)
    .split(' ')
    .filter((w) => w.length > 0)
  if (words.length === 0) return recognized.trim().length > 0 ? 1 : 0
  const haystack = ` ${normalizeForCompare(recognized)} `
  let hits = 0
  for (const word of words) {
    if (haystack.includes(` ${word} `)) hits += 1
  }
  return hits / words.length
}

/** How many of the given number words appear as whole tokens in `text`. */
export function countNumberWordsInTranscript(
  text: string,
  numberWords: readonly string[] = NUMBER_WORDS_ONE_TO_TWENTY,
): number {
  const haystack = ` ${normalizeForCompare(text)} `
  let hits = 0
  for (const word of numberWords) {
    if (haystack.includes(` ${word} `)) hits += 1
  }
  return hits
}

export function evaluateCountingRoundtrip(params: {
  phrase: string
  recognized: string
  stats: UtteranceEventStats
  minNumberWords?: number
  numberWords?: readonly string[]
  label?: string
}): CountingRoundtripResult {
  const numberWords = params.numberWords ?? NUMBER_WORDS_ONE_TO_TWENTY
  const minNumberWords = params.minNumberWords ?? DEFAULT_MIN_NUMBER_WORDS
  const totalWords = numberWords.length
  const failures: string[] = []
  const recognized = params.recognized.trim()

  const who = params.label ? `${params.label}: ` : ''
  if (params.stats.finals.length !== 1) {
    failures.push(
      `${who}expected exactly 1 user_speech_final, got ${params.stats.finals.length}: ${params.stats.finals.map((t) => JSON.stringify(t)).join(', ')}`,
    )
  }
  if (params.stats.speakingEndCount !== 1) {
    failures.push(
      `${who}expected exactly 1 user_speaking_end, got ${params.stats.speakingEndCount}`,
    )
  }
  if (params.stats.speakingStartCount < 1) {
    failures.push(
      `${who}expected at least 1 user_speaking_start, got ${params.stats.speakingStartCount}`,
    )
  }
  if (!recognized) {
    failures.push(`${who}recognized transcript is empty`)
  }

  const numberWordsFound = countNumberWordsInTranscript(recognized, numberWords)
  if (numberWordsFound < minNumberWords) {
    failures.push(
      `${who}expected at least ${minNumberWords}/${totalWords} number words in final, found ${numberWordsFound}`,
    )
  }

  return {
    phrase: params.phrase,
    recognized,
    stats: params.stats,
    numberWordsFound,
    passed: failures.length === 0,
    failures,
  }
}

/**
 * Records listener speech events and resolves the next final transcript (roundtrip pattern).
 */
export class ListenerUtteranceCollector {
  private lastPartial = ''
  private settled = true
  private postSpeechTimer: ReturnType<typeof setTimeout> | undefined
  private progressTimer: ReturnType<typeof setInterval> | undefined
  private resolve: ((text: string) => void) | null = null
  private reject: ((error: Error) => void) | null = null
  private overallTimer: ReturnType<typeof setTimeout> | undefined
  private finalizeWaitMs = 3800
  readonly stats: UtteranceEventStats = {
    finals: [],
    speakingEndCount: 0,
    speakingStartCount: 0,
    partialCount: 0,
    bargeInCount: 0,
    agentSpeakingStartCount: 0,
    agentSpeakingEndCount: 0,
    speakingEndAtMs: null,
    speechFinalAtMs: null,
  }
  private agentSpeakingWaiters: Array<{ baseline: number; resolve: () => void }> = []
  private agentSpeakingEndWaiters: Array<{ baseline: number; resolve: () => void }> = []
  private bargeInWaiters: Array<{ baseline: number; resolve: () => void }> = []
  private eventRecordingStartMs: number | null = null
  private eventRecording: CollectedSpeechEvent[] = []
  private finalAfterBaselineWaiters: Array<{
    baseline: number
    resolve: (text: string) => void
    reject: (error: Error) => void
    timer: ReturnType<typeof setTimeout>
  }> = []

  constructor(
    private readonly listener: VoiceAgent,
    private readonly pumpStarted: { value: boolean },
    private readonly verbose: boolean,
    private readonly agentLabel = 'listener',
    private readonly agentEndLatch?: AgentSpeakingEndLatch,
  ) {}

  startPump(): void {
    if (this.pumpStarted.value) return
    this.pumpStarted.value = true
    void this.pump()
  }

  /** Record speech events with relative timestamps until `stopEventRecording()`. */
  startEventRecording(): void {
    this.eventRecording = []
    this.eventRecordingStartMs = Date.now()
  }

  stopEventRecording(): CollectedSpeechEvent[] {
    this.eventRecordingStartMs = null
    return [...this.eventRecording]
  }

  resetStatsForUtterance(): void {
    this.stats.finals = []
    this.stats.speakingEndCount = 0
    this.stats.speakingStartCount = 0
    this.stats.partialCount = 0
    this.stats.bargeInCount = 0
    this.stats.agentSpeakingStartCount = 0
    this.stats.agentSpeakingEndCount = 0
    this.stats.speakingEndAtMs = null
    this.stats.speechFinalAtMs = null
  }

  /** Resolve when `agent_speaking_end` count exceeds `baseline` (capture via `agentSpeakingEndCount()` before TTS). */
  waitForAgentSpeakingEndAfter(baseline: number, timeoutMs: number): Promise<void> {
    if (this.stats.agentSpeakingEndCount > baseline) {
      return Promise.resolve()
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.agentSpeakingEndWaiters = this.agentSpeakingEndWaiters.filter((w) => w !== waiter)
        reject(new Error(`timed out waiting for agent_speaking_end (${timeoutMs} ms)`))
      }, timeoutMs)
      const waiter = {
        baseline,
        resolve: () => {
          clearTimeout(timer)
          resolve()
        },
      }
      this.agentSpeakingEndWaiters.push(waiter)
    })
  }

  agentSpeakingEndCount(): number {
    return this.stats.agentSpeakingEndCount
  }

  waitForAgentSpeakingEnd(timeoutMs: number): Promise<void> {
    return this.waitForAgentSpeakingEndAfter(this.stats.agentSpeakingEndCount, timeoutMs)
  }

  /** Resolve on the **next** `agent_speaking_start` after this call (not a stale count from an earlier leg). */
  waitForAgentSpeakingStart(timeoutMs: number): Promise<void> {
    const baseline = this.stats.agentSpeakingStartCount
    if (this.stats.agentSpeakingStartCount > baseline) {
      return Promise.resolve()
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.agentSpeakingWaiters = this.agentSpeakingWaiters.filter((w) => w !== waiter)
        reject(new Error(`timed out waiting for agent_speaking_start (${timeoutMs} ms)`))
      }, timeoutMs)
      const waiter = {
        baseline,
        resolve: () => {
          clearTimeout(timer)
          resolve()
        },
      }
      this.agentSpeakingWaiters.push(waiter)
    })
  }

  /** Resolve when `stats.finals.length` exceeds `baseline` (does not reset stats or block `waitForNext`). */
  waitForFinalAfterBaseline(baseline: number, timeoutMs: number): Promise<string> {
    if (this.stats.finals.length > baseline) {
      return Promise.resolve(this.stats.finals[this.stats.finals.length - 1]!)
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.finalAfterBaselineWaiters = this.finalAfterBaselineWaiters.filter((w) => w !== waiter)
        reject(
          new Error(
            `timed out waiting for user_speech_final after baseline ${baseline} (${timeoutMs} ms)`,
          ),
        )
      }, timeoutMs)
      const waiter = { baseline, resolve, reject, timer }
      this.finalAfterBaselineWaiters.push(waiter)
    })
  }

  /** Resolve on the **next** `barge_in` after this call (mid-playback interrupt confirmation). */
  waitForBargeIn(timeoutMs: number): Promise<void> {
    const baseline = this.stats.bargeInCount
    if (this.stats.bargeInCount > baseline) {
      return Promise.resolve()
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.bargeInWaiters = this.bargeInWaiters.filter((w) => w !== waiter)
        reject(new Error(`timed out waiting for barge_in (${timeoutMs} ms)`))
      }, timeoutMs)
      const waiter = {
        baseline,
        resolve: () => {
          clearTimeout(timer)
          resolve()
        },
      }
      this.bargeInWaiters.push(waiter)
    })
  }

  private resolveBaselineWaiters(
    waiters: Array<{ baseline: number; resolve: () => void }>,
    count: number,
  ): void {
    for (let i = waiters.length - 1; i >= 0; i--) {
      const waiter = waiters[i]!
      if (count > waiter.baseline) {
        waiter.resolve()
        waiters.splice(i, 1)
      }
    }
  }

  private recordEvent(event: SpeechEvent): void {
    switch (event.type as SpeechEventType) {
      case 'user_speech_final': {
        this.stats.finals.push((event.text ?? '').trim())
        if (this.stats.speechFinalAtMs == null) {
          this.stats.speechFinalAtMs = Date.now()
        }
        for (let i = this.finalAfterBaselineWaiters.length - 1; i >= 0; i--) {
          const waiter = this.finalAfterBaselineWaiters[i]!
          if (this.stats.finals.length > waiter.baseline) {
            clearTimeout(waiter.timer)
            waiter.resolve(this.stats.finals[this.stats.finals.length - 1]!)
            this.finalAfterBaselineWaiters.splice(i, 1)
          }
        }
        break
      }
      case 'user_speaking_end':
        this.stats.speakingEndCount += 1
        if (this.stats.speakingEndAtMs == null) {
          this.stats.speakingEndAtMs = Date.now()
        }
        break
      case 'user_speaking_start':
        this.stats.speakingStartCount += 1
        break
      case 'user_speech_partial':
        this.stats.partialCount += 1
        break
      case 'barge_in':
        this.stats.bargeInCount += 1
        this.resolveBaselineWaiters(this.bargeInWaiters, this.stats.bargeInCount)
        break
      case 'agent_speaking_start':
        this.stats.agentSpeakingStartCount += 1
        this.resolveBaselineWaiters(this.agentSpeakingWaiters, this.stats.agentSpeakingStartCount)
        break
      case 'agent_speaking_end':
        this.stats.agentSpeakingEndCount += 1
        this.resolveBaselineWaiters(this.agentSpeakingEndWaiters, this.stats.agentSpeakingEndCount)
        break
      default:
        break
    }
  }

  private async pump(): Promise<void> {
    try {
      for await (const event of this.listener.speechEvents()) {
        logRoundtripSpeechEvent(this.agentLabel, event)
        this.agentEndLatch?.observe(event)
        if (this.eventRecordingStartMs != null) {
          this.eventRecording.push({
            type: event.type as SpeechEventType,
            atMs: Date.now() - this.eventRecordingStartMs,
            text: event.text,
          })
        }
        this.recordEvent(event)
        if (this.settled) continue

        if (event.type === 'user_speech_partial' && event.text?.trim()) {
          this.lastPartial = event.text.trim()
        }
        if (event.type === 'user_speech_final') {
          this.finish(event.text ?? this.lastPartial, 'final')
        }
        if (event.type === 'user_speaking_end') {
          this.schedulePartialFallback()
        }
      }
    } catch (error) {
      if (!this.settled && this.reject) {
        this.fail(error instanceof Error ? error : new Error(String(error)))
      }
    }
  }

  waitForNext(timeoutMs: number, finalizeWaitMs: number): Promise<string> {
    if (!this.settled) {
      return Promise.reject(new Error('Previous transcript wait still active'))
    }
    this.resetStatsForUtterance()
    this.settled = false
    this.lastPartial = ''
    this.finalizeWaitMs = finalizeWaitMs
    if (this.postSpeechTimer) clearTimeout(this.postSpeechTimer)
    if (this.progressTimer) clearInterval(this.progressTimer)

    const waitStartedAt = Date.now()
    this.progressTimer = setInterval(() => {
      if (this.settled) return
      const elapsed = Date.now() - waitStartedAt
      console.error(
        `[listener] still waiting for transcript (${(elapsed / 1000).toFixed(0)}s): ` +
          `partials=${this.stats.partialCount} finals=${this.stats.finals.length} ` +
          `speaking_end=${this.stats.speakingEndCount} lastPartial=${JSON.stringify(this.lastPartial.slice(0, 60))}`,
      )
    }, 10_000)
    this.progressTimer.unref()

    return new Promise((resolve, reject) => {
      this.resolve = resolve
      this.reject = reject
      this.overallTimer = setTimeout(() => {
        const fallback = this.lastPartial.trim()
        if (fallback) {
          this.finish(fallback, 'timeout — using last partial')
          return
        }
        console.error(
          `[listener] TIMEOUT after ${timeoutMs} ms waiting for STT transcript (last partial: ${JSON.stringify(this.lastPartial.slice(0, 80))})`,
        )
        this.fail(new Error(`Timed out after ${timeoutMs} ms waiting for STT transcript`))
      }, timeoutMs)
    })
  }

  private schedulePartialFallback(): void {
    if (this.postSpeechTimer) clearTimeout(this.postSpeechTimer)
    this.postSpeechTimer = setTimeout(() => {
      const fallback = this.lastPartial.trim()
      if (!fallback || this.settled) return
      this.finish(fallback, `post-speech fallback after ${this.finalizeWaitMs} ms (no final yet)`)
    }, this.finalizeWaitMs)
  }

  private finish(text: string, reason: string): void {
    if (this.settled) return
    this.settled = true
    if (this.overallTimer) clearTimeout(this.overallTimer)
    if (this.postSpeechTimer) clearTimeout(this.postSpeechTimer)
    if (this.progressTimer) clearInterval(this.progressTimer)
    if (reason !== 'final' && this.verbose) {
      console.log(`[listener] [STT] ${reason}: "${text.trim()}"`)
    }
    this.resolve?.(text.trim())
    this.resolve = null
    this.reject = null
  }

  private fail(error: Error): void {
    if (this.settled) return
    this.settled = true
    if (this.overallTimer) clearTimeout(this.overallTimer)
    if (this.postSpeechTimer) clearTimeout(this.postSpeechTimer)
    if (this.progressTimer) clearInterval(this.progressTimer)
    this.reject?.(error)
    this.resolve = null
    this.reject = null
  }
}

async function main(): Promise<void> {
  installRoundtripWallClockTimeout(55_000)
  const phrase = process.env.SHERPA_COUNTING_PHRASE?.trim() || DEFAULT_COUNTING_PHRASE
  const { config, label, sttModelPath, ttsModelPath } = resolveVoiceConfig()
  const timeoutMs = Number(process.env.SHERPA_COUNTING_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
  const minNumberWords = Number(
    process.env.SHERPA_COUNTING_MIN_NUMBER_WORDS ?? DEFAULT_MIN_NUMBER_WORDS,
  )
  const finalizeWaitMs = sttFinalizeWaitMs(config)
  const postTtsSilenceS = postTtsSilenceSeconds(config)
  const verbose = process.env.SHERPA_COUNTING_VERBOSE === '1'

  console.log('=== Sherpa counting roundtrip (single long utterance) ===')
  console.log(`Pipeline: ${label}`)
  console.log(
    `Listener: gateStt=${config.vad?.gateStt !== false}  minSilence=${config.vad?.minSilenceDurationMs ?? VOICE_AGENT_VAD_PRESET.minSilenceDurationMs}ms  sttGateHold=${config.vad?.sttGateHoldMs ?? VOICE_AGENT_VAD_PRESET.sttGateHoldMs}ms`,
  )
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log(`Phrase length: ${phrase.split(/\s+/).length} words`)
  console.log(
    `Assertions: 1× user_speech_final, 1× user_speaking_end, ≥${minNumberWords}/20 number words`,
  )
  console.log(`Timing: postTtsSilence=${postTtsSilenceS.toFixed(1)}s  timeout=${timeoutMs}ms`)
  console.log('')

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

  const warmupS = Number(process.env.SHERPA_ROUNDTRIP_WARMUP_S ?? DEFAULT_WARMUP_S)
  await streamSilence(agentOut, warmupS)

  const pumpStarted = { value: false }
  const collector = new ListenerUtteranceCollector(listener, pumpStarted, verbose)
  collector.startPump()
  const agentEndLatch = new AgentSpeakingEndLatch()
  startSpeakerSpeechPump(speaker, agentEndLatch)

  console.log('[speaker] Synthesizing long counting phrase…')
  collector.startEventRecording()
  const recognizedPromise = collector.waitForNext(timeoutMs, finalizeWaitMs)
  await playSpeakerTtsWithPostSilence({
    speaker,
    speakerOut: agentOut,
    phrase,
    postTtsSilenceS,
    playbackTimeoutMs: DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
    agentSpeakingEndLatch: agentEndLatch,
  })
  const recognized = await recognizedPromise
  const lifecycleEvents = collector.stopEventRecording()

  const evaluation = evaluateCountingRoundtrip({
    phrase,
    recognized,
    stats: collector.stats,
    minNumberWords,
  })

  console.log('')
  console.log('=== Results ===')
  console.log(`Recognized: "${evaluation.recognized}"`)
  console.log(
    `Events: finals=${evaluation.stats.finals.length} speaking_end=${evaluation.stats.speakingEndCount} speaking_start=${evaluation.stats.speakingStartCount} partials=${evaluation.stats.partialCount}`,
  )
  console.log(`Number words in final: ${evaluation.numberWordsFound}/20`)

  const lifecycleEval = evaluateNormalUtteranceLifecycle({
    events: lifecycleEvents,
    label: 'listener',
  })
  if (!lifecycleEval.passed) {
    evaluation.failures.push(...lifecycleEval.failures)
    evaluation.passed = false
  }

  await listener.stop().catch(() => undefined)
  await speaker.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  if (!evaluation.passed) {
    exitSherpaRoundtripFailure({
      reason: 'counting leg assertions failed',
      failures: evaluation.failures,
      legs: [
        {
          label: 'listener',
          phrase,
          recognized: evaluation.recognized,
          stats: evaluation.stats,
        },
      ],
    })
  }

  console.log('\nCounting roundtrip OK — one final, one speaking_end, numbers captured.')
  process.exit(0)
}

const isMain = process.argv[1]?.endsWith('roundtrip-counting.ts') === true

if (isMain) {
  main().catch((error: unknown) => {
    exitSherpaRoundtripFailure({
      reason: 'uncaught error',
      error,
    })
  })
}
