/**
 * Node-only Sherpa TTS → STT roundtrip (two VoiceAgents, no self-echo).
 *
 * Two peers on one machine (WebRTC + signaling), each with its own VoiceAgent:
 *
 *   Speaker (agent PC)                Listener (user PC)
 *   ─────────────────                 ──────────────────
 *   text → Sherpa TTS → agentOut  →  userInbound → VAD + gateStt → Sherpa STT → text
 *
 * Run (default: 5 built-in sentences + word similarity):
 *   npm run start:roundtrip --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Run one phrase:
 *   npm run start:roundtrip --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa -- "I love America"
 *
 * Optional env:
 *   SHERPA_ROUNDTRIP_PHRASE           single phrase (overrides batch when set)
 *   SHERPA_ROUNDTRIP_TIMEOUT_MS       per-phrase STT timeout (default 45000)
 *   SHERPA_ROUNDTRIP_WARMUP_S         speaker warmup silence before first TTS (default 0.6)
 *   SHERPA_ROUNDTRIP_MIN_SIMILARITY   min word match ratio 0–1 (default 0.75)
 *   SHERPA_ROUNDTRIP_GAP_S            extra silence between phrases (default 0 — see ROUNDTRIP.md)
 *
 * Full documentation: examples/voice-agent-local-sherpa/ROUNDTRIP.md
 */

import type { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'
import type { SpeechEvent, VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { streamSilence } from './pcm-relay.js'
import { resolveVoiceConfig } from './resolve-voice-config.js'

/** Default batch when no argv / SHERPA_ROUNDTRIP_PHRASE. */
const DEFAULT_SENTENCES = [
  'I love America',
  'The weather is nice today.',
  'Hello world',
  'Open the browser please',
  'Speech recognition works locally',
]

const DEFAULT_TIMEOUT_MS = 45_000
/** Added to sttGateHoldMs + endpoint tail when computing post-TTS silence duration. */
const FINALIZE_MARGIN_MS = 500
const DEFAULT_WARMUP_S = 0.6
/** Extra inter-phrase silence (seconds). Default 0 — VAD hold + VAD-aligned post-TTS trailing silence suffice. */
const DEFAULT_GAP_S = 0
const DEFAULT_MIN_SIMILARITY = 0.75

interface RoundtripPhraseResult {
  input: string
  recognized: string
  normalizedInput: string
  normalizedRecognized: string
  similarity: number
  passed: boolean
}

function resolvePhrases(): string[] {
  const fromArg = process.argv.slice(2).join(' ').trim()
  if (fromArg) return [fromArg]
  const fromEnv = process.env.SHERPA_ROUNDTRIP_PHRASE?.trim()
  if (fromEnv) return [fromEnv]
  return DEFAULT_SENTENCES
}

function endpointTailMs(config: VoiceAgentConfig): number {
  return Math.max(config.vad?.minSilenceDurationMs ?? 300, 800)
}

/** Ms to wait after VAD speech end before accepting a partial (aligns with hold + tail + margin). */
function sttFinalizeWaitMs(config: VoiceAgentConfig): number {
  const hold = config.vad?.sttGateHoldMs ?? 2500
  return hold + endpointTailMs(config) + FINALIZE_MARGIN_MS
}

/**
 * Trailing silence on speaker outbound after each TTS utterance.
 * Duration matches listener `sttGateHoldMs` + endpoint tail so hold can drain on the wire.
 */
function postTtsSilenceSeconds(config: VoiceAgentConfig): number {
  const hold = config.vad?.sttGateHoldMs ?? 2500
  return (hold + endpointTailMs(config) + FINALIZE_MARGIN_MS) / 1000
}

/** Lowercase, strip punctuation, collapse spaces — for similarity only. */
function normalizeForCompare(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9\s]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}

/**
 * Share of input words (after normalize) found as whole tokens in recognized text.
 * 1.0 = every input word appears in the STT transcript.
 */
function wordSimilarity(input: string, recognized: string): number {
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

function logSpeechEvent(role: 'speaker' | 'listener', event: SpeechEvent, verbose: boolean): void {
  if (!verbose) return
  const tag = role === 'speaker' ? '[speaker]' : '[listener]'
  switch (event.type) {
    case 'user_speech_partial':
      console.log(`${tag} [STT partial] ${event.text ?? ''}`)
      break
    case 'user_speech_final':
      console.log(`${tag} [STT final] ${event.text ?? ''}`)
      break
    case 'user_speaking_start':
      console.log(`${tag} [VAD] speech start`)
      break
    case 'user_speaking_end':
      console.log(`${tag} [VAD] speech end`)
      break
    case 'agent_speaking_start':
      console.log(`${tag} [TTS] playback start`)
      break
    case 'agent_speaking_end':
      console.log(`${tag} [TTS] playback end`)
      break
    case 'barge_in':
      console.log(`${tag} [barge-in]`)
      break
    case 'error':
      console.error(`${tag} [error]`, event.error ?? event)
      break
    default:
      break
  }
}

/** One-shot wait for the next listener transcript (reused across batch phrases). */
class ListenerTranscriptCollector {
  private lastPartial = ''
  private settled = true
  private postSpeechTimer: ReturnType<typeof setTimeout> | undefined
  private resolve: ((text: string) => void) | null = null
  private reject: ((error: Error) => void) | null = null
  private overallTimer: ReturnType<typeof setTimeout> | undefined
  private finalizeWaitMs = 3800
  private readonly verbose: boolean

  constructor(
    private readonly listener: VoiceAgent,
    private readonly pumpStarted: { value: boolean },
  ) {
    this.verbose = process.env.SHERPA_ROUNDTRIP_VERBOSE === '1'
  }

  startPump(): void {
    if (this.pumpStarted.value) return
    this.pumpStarted.value = true
    void this.pump()
  }

  private async pump(): Promise<void> {
    try {
      for await (const event of this.listener.speechEvents()) {
        if (this.settled) continue
        logSpeechEvent('listener', event, this.verbose)

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
    this.settled = false
    this.lastPartial = ''
    this.finalizeWaitMs = finalizeWaitMs
    if (this.postSpeechTimer) clearTimeout(this.postSpeechTimer)

    return new Promise((resolve, reject) => {
      this.resolve = resolve
      this.reject = reject
      this.overallTimer = setTimeout(() => {
        const fallback = this.lastPartial.trim()
        if (fallback) {
          this.finish(fallback, 'timeout — using last partial')
          return
        }
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
    this.reject?.(error)
    this.resolve = null
    this.reject = null
  }
}

async function runPhrase(params: {
  phrase: string
  index: number
  total: number
  speaker: VoiceAgent
  speakerOut: LocalAudioTrack
  collector: ListenerTranscriptCollector
  timeoutMs: number
  finalizeWaitMs: number
  postTtsSilenceS: number
  minSimilarity: number
  gapS: number
}): Promise<RoundtripPhraseResult> {
  const {
    phrase,
    index,
    total,
    speaker,
    speakerOut,
    collector,
    timeoutMs,
    finalizeWaitMs,
    postTtsSilenceS,
    minSimilarity,
    gapS,
  } = params

  console.log('')
  console.log(`--- Phrase ${index + 1}/${total} ---`)
  console.log(`Input: "${phrase}"`)

  const recognizedPromise = collector.waitForNext(timeoutMs, finalizeWaitMs)

  console.log('[speaker] Synthesizing…')
  await speaker.sendTextToTTS(phrase)
  // Trailing silence at real-time frame rate, in parallel with STT finalize (like post-utterance quiet on a call).
  const recognized = await Promise.all([
    streamSilence(speakerOut, postTtsSilenceS),
    recognizedPromise,
  ]).then(([, text]) => text)
  const normalizedInput = normalizeForCompare(phrase)
  const normalizedRecognized = normalizeForCompare(recognized)
  const similarity = wordSimilarity(phrase, recognized)
  const passed = recognized.trim().length > 0 && similarity >= minSimilarity

  console.log(`Recognized:  "${recognized}"`)
  console.log(`Normalized:  "${normalizedInput}" → "${normalizedRecognized}"`)
  console.log(
    `Similarity:  ${(similarity * 100).toFixed(0)}% (min ${(minSimilarity * 100).toFixed(0)}%)`,
  )
  console.log(passed ? 'Phrase OK' : 'Phrase FAILED')

  if (gapS > 0 && index + 1 < total) {
    await streamSilence(speakerOut, gapS)
  }

  return {
    input: phrase,
    recognized,
    normalizedInput,
    normalizedRecognized,
    similarity,
    passed,
  }
}

async function main(): Promise<void> {
  const phrases = resolvePhrases()
  const { config, label, sttModelPath, ttsModelPath } = resolveVoiceConfig()
  const timeoutMs = Number(process.env.SHERPA_ROUNDTRIP_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
  const finalizeWaitMs = sttFinalizeWaitMs(config)
  const postTtsSilenceS = postTtsSilenceSeconds(config)
  const minSimilarity = Number(
    process.env.SHERPA_ROUNDTRIP_MIN_SIMILARITY ?? DEFAULT_MIN_SIMILARITY,
  )
  const gapS = Number(process.env.SHERPA_ROUNDTRIP_GAP_S ?? DEFAULT_GAP_S)
  const verbose = process.env.SHERPA_ROUNDTRIP_VERBOSE === '1'

  console.log('=== Sherpa TTS → STT roundtrip (two VoiceAgents) ===')
  console.log(`Pipeline: ${label}`)
  console.log(
    `Listener: gateStt=${config.vad?.gateStt !== false}  minSilence=${config.vad?.minSilenceDurationMs ?? 300}ms  sttGateHold=${config.vad?.sttGateHoldMs ?? 2500}ms  bargeIn=${config.vad?.bargeIn?.enabled !== false}`,
  )
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log(`Phrases: ${phrases.length}  minSimilarity=${minSimilarity}`)
  console.log(
    `Timing: postTtsSilence=${postTtsSilenceS.toFixed(1)}s (from VAD hold+tail)  interPhraseGap=${gapS}s (0=VAD only)`,
  )
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
  const collector = new ListenerTranscriptCollector(listener, pumpStarted)
  collector.startPump()

  if (verbose) {
    void (async () => {
      for await (const event of speaker.speechEvents()) {
        logSpeechEvent('speaker', event, true)
      }
    })()
  }

  const results: RoundtripPhraseResult[] = []
  for (let i = 0; i < phrases.length; i += 1) {
    results.push(
      await runPhrase({
        phrase: phrases[i]!,
        index: i,
        total: phrases.length,
        speaker,
        speakerOut: agentOut,
        collector,
        timeoutMs,
        finalizeWaitMs,
        postTtsSilenceS,
        minSimilarity,
        gapS,
      }),
    )
  }

  console.log('')
  console.log('=== Summary ===')
  console.log('| # | Similarity | OK | Input | Recognized |')
  console.log('|---|------------|-----|-------|------------|')
  for (let i = 0; i < results.length; i += 1) {
    const r = results[i]!
    const ok = r.passed ? 'yes' : 'NO'
    const pct = `${(r.similarity * 100).toFixed(0)}%`
    console.log(`| ${i + 1} | ${pct} | ${ok} | ${r.input} | ${r.recognized} |`)
  }

  const failed = results.filter((r) => !r.passed)
  await listener.stop().catch(() => undefined)
  await speaker.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  if (failed.length > 0) {
    console.error(
      `\nRoundtrip failed: ${failed.length}/${results.length} phrase(s) below similarity threshold.`,
    )
    process.exit(1)
  }

  console.log(`\nRoundtrip OK — ${results.length} phrase(s) passed similarity check.`)
  process.exit(0)
}

main().catch((error: unknown) => {
  console.error(error)
  process.exit(1)
})
