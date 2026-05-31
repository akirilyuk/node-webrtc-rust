/**
 * E2E barge-in test (Sherpa TTS) — same loopback layout as roundtrip.ts.
 *
 * Barge-in: speaker has vad.enabled + bargeIn { enabled, useVad: true, flushTts }.
 * Inbound VAD SpeechStart on agentInbound flushes TTS and emits barge_in.
 * Set useVad: false to interrupt only via flushTts() (no auto interrupt on tones).
 *
 *   Speaker (agent PC)                         User leg
 *   ─────────────────                          ────────
 *   Sherpa TTS → agentOut  →  userInbound      userOut → agentInbound (interrupt tone)
 *   VAD + bargeIn on agentInbound              SpeechStart → barge_in + TTS flush
 *
 * Phase 1: play a long phrase with no interrupt → measure full playback time on userInbound.
 * Phase 2: same phrase, inject user tone mid-TTS → playback must be shorter; barge_in required.
 *
 * Run:
 *   npm run start:roundtrip-barge-in --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Env:
 *   SHERPA_BARGE_IN_PHRASE          TTS text (default long English sentence)
 *   SHERPA_BARGE_IN_DELAY_MS        ms after TTS start before interrupt (default 900)
 *   SHERPA_BARGE_IN_INTERRUPT_S     seconds of user tone at real-time pace (default 1.2)
 *   SHERPA_BARGE_IN_MAX_RATIO       cut/full playback ratio must be below this (default 0.65)
 *   SHERPA_BARGE_IN_VERBOSE         set to 1 for speech events
 *
 * See ROUNDTRIP.md § Barge-in E2E.
 */

import type { LocalAudioTrack, RemoteAudioTrack } from '@node-webrtc-rust/sdk'
import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'
import type { SpeechEvent, VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { stereoPcmDurationMs, streamSilence, streamTone } from './pcm-relay.js'
import { resolveVoiceConfig } from './resolve-voice-config.js'

/** Long enough that mid-utterance interrupt clearly shortens received audio. */
const DEFAULT_PHRASE =
  'The quick brown fox jumps over the lazy dog and then continues speaking for several more seconds so we can interrupt playback.'

/** Start user tone with TTS so VAD can barge during synthesis/drain (not after). */
const DEFAULT_BARGE_DELAY_MS = 0
const DEFAULT_INTERRUPT_S = 1.2
const DEFAULT_MAX_RATIO = 0.65
const DEFAULT_WARMUP_S = 0.6

function speakerVadConfig(): NonNullable<VoiceAgentConfig['vad']> {
  return {
    enabled: true,
    threshold: 0.05,
    // E2E: low enough that interrupt tone triggers SpeechStart during TTS; production often uses 200–300.
    minSpeechDurationMs: 40,
    minSilenceDurationMs: 300,
    speechPadMs: 300,
    gateStt: false,
    bargeIn: { enabled: true, useVad: true, flushTts: true },
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/** Sums durationMs of non-empty PCM read from the remote TTS track. */
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

function logSpeechEvent(role: string, event: SpeechEvent, verbose: boolean): void {
  if (!verbose) return
  const extra = event.text ? ` text="${event.text}"` : ''
  console.log(`[${role}] ${event.type}${extra}`)
}

/** Both events follow inbound VAD SpeechStart; Rust may emit `barge_in` before `user_speaking_start`. */
function waitForBargeInSequence(
  speaker: VoiceAgent,
  timeoutMs: number,
  verbose: boolean,
): Promise<{ sawSpeechStart: boolean; sawBargeIn: boolean }> {
  return new Promise((resolve, reject) => {
    let sawSpeechStart = false
    let sawBargeIn = false

    const timer = setTimeout(() => {
      reject(
        new Error(
          `Timed out after ${timeoutMs} ms (user_speaking_start=${sawSpeechStart}, barge_in=${sawBargeIn})`,
        ),
      )
    }, timeoutMs)

    const maybeDone = () => {
      if (sawSpeechStart && sawBargeIn) {
        clearTimeout(timer)
        resolve({ sawSpeechStart, sawBargeIn })
      }
    }

    void (async () => {
      try {
        for await (const event of speaker.speechEvents()) {
          logSpeechEvent('speaker', event, verbose)
          if (event.type === 'user_speaking_start') {
            sawSpeechStart = true
            maybeDone()
          }
          if (event.type === 'barge_in') {
            sawBargeIn = true
            maybeDone()
          }
        }
        clearTimeout(timer)
        reject(
          new Error(
            `speech event stream ended (user_speaking_start=${sawSpeechStart}, barge_in=${sawBargeIn})`,
          ),
        )
      } catch (error) {
        clearTimeout(timer)
        reject(error)
      }
    })()
  })
}

async function runBargeInPlayback(params: {
  speaker: VoiceAgent
  phrase: string
  userInbound: RemoteAudioTrack
  userOut: LocalAudioTrack
  bargeDelayMs: number
  interruptS: number
  timeoutMs: number
  verbose: boolean
}): Promise<number> {
  const meter = new InboundAudioMeter()
  meter.start(params.userInbound)

  const ttsDone = params.speaker.sendTextToTTS(params.phrase)
  const bargeSeen = waitForBargeInSequence(params.speaker, params.timeoutMs, params.verbose)

  const interruptTask = (async () => {
    await sleep(params.bargeDelayMs)
    console.log(`[user] Injecting interrupt tone for ${params.interruptS}s (real-time)…`)
    await streamTone(params.userOut, params.interruptS, 440)
  })()

  await bargeSeen
  meter.stop()
  const receivedBeforeBargeMs = meter.getTotalMs()

  await Promise.all([ttsDone, interruptTask])
  return receivedBeforeBargeMs
}

async function main(): Promise<void> {
  const phrase =
    process.env.SHERPA_BARGE_IN_PHRASE?.trim() ||
    process.argv.slice(2).join(' ').trim() ||
    DEFAULT_PHRASE
  const bargeDelayMs = Number(process.env.SHERPA_BARGE_IN_DELAY_MS ?? DEFAULT_BARGE_DELAY_MS)
  const interruptS = Number(process.env.SHERPA_BARGE_IN_INTERRUPT_S ?? DEFAULT_INTERRUPT_S)
  const maxRatio = Number(process.env.SHERPA_BARGE_IN_MAX_RATIO ?? DEFAULT_MAX_RATIO)
  const verbose = process.env.SHERPA_BARGE_IN_VERBOSE === '1'
  const timeoutMs = Number(process.env.SHERPA_BARGE_IN_TIMEOUT_MS ?? 60_000)

  const { config, label, sttModelPath, ttsModelPath } = resolveVoiceConfig()

  console.log('=== Sherpa barge-in E2E (two VoiceAgents) ===')
  console.log(`Pipeline: ${label}`)
  console.log(`Phrase length: ${phrase.length} chars`)
  console.log(`Interrupt: delay=${bargeDelayMs}ms  tone=${interruptS}s  maxCutRatio=${maxRatio}`)
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log('')

  const { agentOut, userInbound, userOut, agentInbound, cleanup } =
    await createBidirectionalLoopback()

  const speaker = new VoiceAgent({
    tts: config.tts,
    events: { mode: 'stream' },
    vad: speakerVadConfig(),
  })

  await speaker.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await speaker.start()

  await streamSilence(agentOut, DEFAULT_WARMUP_S)

  console.log('--- Phase 1: full playback (no interrupt) ---')
  const meterFull = new InboundAudioMeter()
  meterFull.start(userInbound)
  console.log('[speaker] Synthesizing full phrase…')
  await speaker.sendTextToTTS(phrase)
  meterFull.stop()
  const fullMs = meterFull.getTotalMs()
  console.log(`Received on userInbound: ${fullMs} ms (full)`)

  if (fullMs < 500) {
    console.error('Full playback too short — check WebRTC loopback and Sherpa TTS.')
    process.exit(1)
  }

  await streamSilence(agentOut, 0.5)

  console.log('')
  console.log('--- Phase 2: barge-in mid playback ---')
  console.log('[speaker] Synthesizing (will interrupt)…')
  const cutMs = await runBargeInPlayback({
    speaker,
    phrase,
    userInbound,
    userOut,
    bargeDelayMs,
    interruptS,
    timeoutMs,
    verbose,
  })

  const ratio = cutMs / fullMs
  console.log(`Received on userInbound before barge_in: ${cutMs} ms`)
  console.log(
    `Pre-barge/full ratio: ${(ratio * 100).toFixed(0)}% (must be < ${(maxRatio * 100).toFixed(0)}%)`,
  )
  console.log('user_speaking_start → barge_in: yes (VAD voice activity)')

  await speaker.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  if (ratio >= maxRatio) {
    console.error(
      `\nBarge-in E2E FAILED: interrupted playback was not short enough (${cutMs} ms vs full ${fullMs} ms).`,
    )
    process.exit(1)
  }

  console.log('\nBarge-in E2E OK — TTS playback was truncated after user interrupt.')
  process.exit(0)
}

main().catch((error: unknown) => {
  console.error(error)
  process.exit(1)
})
