/**
 * VoiceAgent stream mode demo (pullSpeechEvent / speechEvents).
 *
 * npm run start:stream --workspace=@node-webrtc-rust/example-voice-agent
 */

import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

import { createLoopbackAudio, mockVoiceConfig } from './shared-loopback.js'

async function main(): Promise<void> {
  const { agentOut, agentInbound, cleanup } = await createLoopbackAudio()

  const agent = new VoiceAgent({
    ...mockVoiceConfig,
    events: { mode: 'stream' },
  })

  await agent.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await agent.start()

  const streamTask = (async () => {
    for await (const event of agent.speechEvents()) {
      console.log('[stream]', event.type, event.text ?? event.error ?? '')
      if (event.type === 'agent_speaking_end') break
    }
  })()

  await agent.sendTextToTTS('Streaming speech events from mock TTS.')
  await Promise.race([streamTask, new Promise((resolve) => setTimeout(resolve, 2000))])

  await agent.stop()
  await cleanup()
  console.log('Voice stream demo complete')
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
