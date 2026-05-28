/**
 * Node-only Sherpa TTS → STT roundtrip (no browser, no mic).
 *
 * Exercises the full on-device speech loop with gateStt + pre-roll:
 *
 *   text ──► local Sherpa TTS ──► agent outbound track
 *              └── WebRTC ──► user inbound (hears agent)
 *                    └── PCM relay ──► user outbound
 *                          └── WebRTC ──► agent inbound ──► local Sherpa STT ──► text
 *
 * Prerequisites:
 *   npm run download-stt:en --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *   npm run download-tts:en --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *   export SHERPA_STT_MODEL_PATH=.../.models/sherpa-onnx-streaming-zipformer-en-2023-06-26
 *   export SHERPA_TTS_MODEL_PATH=.../.models/vits-piper-en_US-amy-low
 *
 * Run:
 *   npm run start:roundtrip --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *   npm run start:roundtrip --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa -- "The weather is nice today."
 *
 * Optional env:
 *   SHERPA_ROUNDTRIP_PHRASE       default phrase when argv is omitted
 *   SHERPA_ROUNDTRIP_TIMEOUT_MS    overall STT wait timeout (default 15000)
 */

import type { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'
import type { SpeechEvent } from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { relayRemoteAudioToLocal, streamSilence } from './pcm-relay.js'
import { resolveVoiceConfig } from './resolve-voice-config.js'

const DEFAULT_PHRASE = 'The weather is nice today.'
const TIMEOUT_MS = Number(process.env.SHERPA_ROUNDTRIP_TIMEOUT_MS ?? 15_000)
/** After VAD speech end, accept the last partial if Sherpa has not emitted final yet. */
const POST_SPEECH_PARTIAL_MS = 1_500
/** Brief silence after TTS so VAD can close the utterance (agent also injects an STT tail). */
const POST_TTS_SILENCE_S = 0.8

function inputPhrase(): string {
  const fromArg = process.argv.slice(2).join(' ').trim()
  const fromEnv = process.env.SHERPA_ROUNDTRIP_PHRASE?.trim()
  return fromArg || fromEnv || DEFAULT_PHRASE
}

function logSpeechEvent(event: SpeechEvent): void {
  switch (event.type) {
    case 'user_speech_partial':
      console.log(`[STT partial] ${event.text ?? ''}`)
      break
    case 'user_speech_final':
      console.log(`[STT final] ${event.text ?? ''}`)
      break
    case 'user_speaking_start':
      console.log('[VAD] speech start')
      break
    case 'user_speaking_end':
      console.log('[VAD] speech end')
      break
    case 'agent_speaking_start':
      console.log('[TTS] playback start')
      break
    case 'agent_speaking_end':
      console.log('[TTS] playback end')
      break
    case 'error':
      console.error('[error]', event.error ?? event)
      break
    default:
      break
  }
}

async function main(): Promise<void> {
  const phrase = inputPhrase()
  const { config, label, sttModelPath, ttsModelPath } = resolveVoiceConfig()

  console.log('=== Sherpa TTS → STT roundtrip (Node loopback, gateStt) ===')
  console.log(`Pipeline: ${label}`)
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log(`Input phrase: "${phrase}"`)
  console.log('')

  const { agentOut, agentInbound, userOut, userInbound, cleanup } =
    await createBidirectionalLoopback()

  const agent = new VoiceAgent({
    ...config,
    events: { mode: 'stream' },
    vad: {
      ...config.vad,
      gateStt: true,
      minSpeechDurationMs: 150,
      minSilenceDurationMs: 400,
      sttGateHoldMs: 3000,
      bargeIn: { enabled: false, flushTts: false },
    },
  })

  let relayActive = true
  void relayRemoteAudioToLocal(userInbound, userOut, () => relayActive)

  await agent.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await agent.start()

  const recognizedPromise = collectSttTranscript(agent, userOut, TIMEOUT_MS)

  console.log('Synthesizing with local Sherpa TTS…')
  await agent.sendTextToTTS(phrase)

  console.log(`Waiting up to ${TIMEOUT_MS} ms for STT (event-driven, gateStt)…`)
  const recognized = await recognizedPromise

  relayActive = false

  console.log('')
  console.log('--- Roundtrip result ---')
  console.log(`Input:      "${phrase}"`)
  console.log(`Recognized: "${recognized}"`)

  if (!recognized.trim()) {
    console.error('Roundtrip failed: empty STT transcript.')
    process.exit(1)
  }

  console.log('Roundtrip OK (non-empty STT transcript).')

  void agent.stop().catch(() => undefined)
  void cleanup().catch(() => undefined)
  process.exit(0)
}

function collectSttTranscript(
  agent: VoiceAgent,
  userOut: LocalAudioTrack,
  timeoutMs: number,
): Promise<string> {
  let lastPartial = ''
  let settled = false
  let postSpeechTimer: ReturnType<typeof setTimeout> | undefined
  let ttsSilenceStarted = false

  const finish = (
    resolve: (text: string) => void,
    clearOverallTimer: () => void,
    text: string,
  ): void => {
    if (settled) return
    settled = true
    clearOverallTimer()
    if (postSpeechTimer) clearTimeout(postSpeechTimer)
    resolve(text.trim())
  }

  return new Promise((resolve, reject) => {
    const overallTimer = setTimeout(() => {
      const fallback = lastPartial.trim()
      if (fallback) {
        console.log(`[STT] timeout — using last partial: "${fallback}"`)
        finish(resolve, () => clearTimeout(overallTimer), fallback)
        return
      }
      reject(new Error(`Timed out after ${timeoutMs} ms waiting for STT transcript`))
    }, timeoutMs)

    const clearOverallTimer = () => clearTimeout(overallTimer)

    const schedulePartialFallback = (): void => {
      if (postSpeechTimer) clearTimeout(postSpeechTimer)
      postSpeechTimer = setTimeout(() => {
        const fallback = lastPartial.trim()
        if (!fallback) return
        console.log(`[STT] post-speech fallback: "${fallback}"`)
        finish(resolve, clearOverallTimer, fallback)
      }, POST_SPEECH_PARTIAL_MS)
    }

    void (async () => {
      try {
        for await (const event of agent.speechEvents()) {
          if (settled) return
          logSpeechEvent(event)

          if (event.type === 'agent_speaking_end' && !ttsSilenceStarted) {
            ttsSilenceStarted = true
            void streamSilence(userOut, POST_TTS_SILENCE_S).catch(() => undefined)
          }

          if (event.type === 'user_speech_partial' && event.text?.trim()) {
            lastPartial = event.text.trim()
          }

          if (event.type === 'user_speech_final') {
            finish(resolve, clearOverallTimer, event.text ?? lastPartial)
            return
          }

          if (event.type === 'user_speaking_end') {
            schedulePartialFallback()
          }
        }
      } catch (error) {
        if (!settled) {
          clearOverallTimer()
          if (postSpeechTimer) clearTimeout(postSpeechTimer)
          reject(error)
        }
      }
    })()
  })
}

main().catch((error: unknown) => {
  console.error(error)
  process.exit(1)
})
