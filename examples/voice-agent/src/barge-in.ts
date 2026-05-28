/**
 * VoiceAgent — **barge-in** demo (mock vendors, no API keys).
 *
 * ## What this shows
 *
 * When the user starts speaking while the agent is playing TTS, native code can:
 *
 * 1. **Flush** the outbound TTS buffer (`bargeIn.flushTts: true`) — instant audio cut
 * 2. Emit **`barge_in`** to Node (`bargeIn.enabled: true`) — your LLM loop can cancel
 *
 * These toggles are independent: set `flushTts: false` to get the event only and decide
 * in JS whether to call `flushTts()`.
 *
 * ## Why inject PCM via processInboundPcm?
 *
 * In production, inbound PCM arrives from `RemoteAudioTrack.readSample()` in a background
 * loop (see `VoiceAgent.start()` in the SDK). This demo bypasses RTP and pushes one loud
 * 20 ms frame directly into the native VAD path to trigger speech-start deterministically.
 *
 * ## VAD threshold note
 *
 * We use a low threshold (0.05) and short `minSpeechDurationMs` so a single loud frame
 * counts as speech in the default energy-based VAD. Silero VAD (`--features silero-vad`)
 * behaves differently — tune thresholds for production.
 *
 * ## Run
 *
 * ```bash
 * npm run start:barge-in --workspace=@node-webrtc-rust/example-voice-agent
 * ```
 *
 * Expected: `[barge-in] TTS flushed` after the synthetic user frame.
 */

import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

import { createLoopbackAudio, mockVoiceConfig } from './shared-loopback.js'

async function main(): Promise<void> {
  const { agentOut, agentInbound, cleanup } = await createLoopbackAudio()

  const agent = new VoiceAgent({
    ...mockVoiceConfig,
    events: { mode: 'both' },
    vad: {
      enabled: true,
      threshold: 0.05,
      minSpeechDurationMs: 20,
      bargeIn: { enabled: true, flushTts: true },
    },
  })

  agent.on('barge_in', () => console.log('[barge-in] TTS flushed'))

  await agent.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await agent.start()

  // Fire-and-forget TTS — we interrupt it before it finishes draining.
  void agent.sendTextToTTS('Long agent utterance that may be interrupted.')

  // 3840 bytes = 20 ms stereo PCM at 48 kHz — same size as a real decoded WebRTC frame.
  const loud = Buffer.alloc(3840)
  for (let i = 0; i < loud.length; i += 2) {
    loud.writeInt16LE(8000, i)
  }

  // Direct native entry point (also used internally by the SDK inbound loop).
  await agent.getNativeAgent().processInboundPcm(loud, 20)

  await new Promise((resolve) => setTimeout(resolve, 300))
  await agent.stop()
  await cleanup()
  console.log('Barge-in demo complete')
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
