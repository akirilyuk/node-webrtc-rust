/**
 * `VoiceAgent.stop()` must resolve promptly while an STT utterance is still open.
 *
 * A "speaker" agent renders Piper TTS onto a loopback peer connection; a listen-only agent
 * (local Sherpa STT + LID + TTS configured, as in a client that transcribes the reply) hears
 * it. Once STT partials arrive and the speaker goes quiet, the listener is stopped at several
 * offsets. Opt-in (needs Sherpa model bundles):
 *
 * ```bash
 * M=examples/voice-agent-local-sherpa/.models
 * VOICE_STOP_TEST=1 \
 * SHERPA_STT_MODEL_PATH=$PWD/$M/sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06 \
 * SHERPA_TTS_MODEL_PATH=$PWD/$M/vits-piper-en_US-amy-low \
 * SHERPA_LID_MODEL_PATH=$PWD/$M/sherpa-onnx-whisper-tiny \
 * npm run test --workspace=@node-webrtc-rust/sdk -- voice-stop-open-utterance
 * ```
 */
import { describe, expect, test } from 'vitest'

import { LocalAudioTrack } from '../src'
import { VoiceAgent, type VoiceAgentConfig } from '../src/voice'
import { createVoiceLoopback } from './voice-helpers'

const enabled = process.env.VOICE_STOP_TEST === '1'
const STOP_BOUND_MS = 10_000
const PHRASE = 'You said one, two, three, four, five, six, seven, eight, nine, ten.'

function listenerConfig(): VoiceAgentConfig {
  return {
    stt: {
      provider: 'local-sherpa',
      language: 'en',
      modelPath: process.env.SHERPA_STT_MODEL_PATH!,
    },
    tts: {
      provider: 'local-sherpa',
      modelPath: process.env.SHERPA_TTS_MODEL_PATH!,
      voice: '0',
    },
    languageId: {
      enabled: true,
      modelPath: process.env.SHERPA_LID_MODEL_PATH!,
      allowlist: ['en', 'de'],
      minSpeechMs: 800,
    },
    events: { mode: 'callback' },
    vad: { provider: 'energy', threshold: 0.05, minSpeechDurationMs: 200, sttGateHoldMs: 1000 },
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

async function stopWithin(agent: VoiceAgent, boundMs: number): Promise<number | null> {
  const started = Date.now()
  const result = await Promise.race([
    agent.stop().then(() => 'stopped' as const),
    sleep(boundMs).then(() => 'timeout' as const),
  ])
  return result === 'stopped' ? Date.now() - started : null
}

describe.skipIf(!enabled)('VoiceAgent.stop with an open STT utterance', () => {
  for (const extraDelayMs of [0, 400, 1300, 3000]) {
    test(
      `stop resolves ${extraDelayMs}ms after the first partial`,
      async () => {
        const { agentOut, userInbound, cleanup } = await createVoiceLoopback()
        const speaker = new VoiceAgent({
          stt: { provider: 'mock', language: 'en' },
          tts: {
            provider: 'local-sherpa',
            modelPath: process.env.SHERPA_TTS_MODEL_PATH!,
            voice: '0',
          },
          events: { mode: 'callback' },
        })
        const listener = new VoiceAgent(listenerConfig())
        const partials: string[] = []
        listener.on('user_speech_partial', (e) => partials.push(e.text ?? ''))
        await speaker.attach({
          inboundTrack: userInbound,
          outboundTrack: agentOut,
        })
        await listener.attach({
          inboundTrack: userInbound,
          outboundTrack: new LocalAudioTrack('listener-out', 'voice-test'),
        })
        await speaker.start()
        await listener.start()
        void speaker.sendTextToTTS(PHRASE, { nonBlocking: true })

        const deadline = Date.now() + 30_000
        while (partials.length === 0 && Date.now() < deadline) await sleep(20)
        expect(partials.length).toBeGreaterThan(0)
        await sleep(extraDelayMs)

        const tookMs = await stopWithin(listener, STOP_BOUND_MS)
        // eslint-disable-next-line no-console
        console.log(`stop after +${extraDelayMs}ms took ${tookMs}ms`)
        expect(tookMs, `listener.stop() hung > ${STOP_BOUND_MS}ms`).not.toBeNull()
        expect(tookMs!).toBeLessThan(5_000)
        await speaker.stop().catch(() => undefined)
        await cleanup()
      },
      90_000,
    )
  }
})
