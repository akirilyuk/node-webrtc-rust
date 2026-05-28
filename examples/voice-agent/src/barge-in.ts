/**
 * Barge-in demo: flush TTS when inbound speech is detected.
 *
 * npm run start:barge-in --workspace=@node-webrtc-rust/example-voice-agent
 */

import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

import { createLoopbackAudio, mockVoiceConfig } from './shared-loopback.js'

async function main(): Promise<void> {
  const { agentOut, userInbound, cleanup } = await createLoopbackAudio()

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

  await agent.attach({ inboundTrack: userInbound, outboundTrack: agentOut })
  await agent.start()

  void agent.sendTextToTTS('Long agent utterance that may be interrupted.')

  // Simulate user speech with loud PCM frame
  const loud = Buffer.alloc(3840)
  for (let i = 0; i < loud.length; i += 2) {
    loud.writeInt16LE(8000, i)
  }
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
