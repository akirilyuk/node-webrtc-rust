/**
 * VoiceAgent callback mode demo.
 *
 * npm run start:callback --workspace=@node-webrtc-rust/example-voice-agent
 */

import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

import { createLoopbackAudio, mockVoiceConfig } from './shared-loopback.js'

async function main(): Promise<void> {
  const { agentOut, agentInbound, cleanup } = await createLoopbackAudio()

  const agent = new VoiceAgent({
    ...mockVoiceConfig,
    events: { mode: 'callback' },
  })

  agent.on('user_speech_final', (event) => {
    console.log('[callback] user_speech_final:', event.text)
  })
  agent.on('agent_speaking_start', () => {
    console.log('[callback] agent_speaking_start')
  })
  agent.on('barge_in', () => {
    console.log('[callback] barge_in')
  })

  await agent.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await agent.start()
  await agent.sendTextToTTS('Hello from the mock voice agent.')
  await new Promise((resolve) => setTimeout(resolve, 500))
  await agent.stop()
  await cleanup()
  console.log('Voice callback demo complete')
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
