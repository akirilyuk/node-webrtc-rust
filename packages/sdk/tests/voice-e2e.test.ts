import { describe, test } from 'vitest'

import { VoiceAgent } from '../src/voice'
import { createVoiceLoopback } from './voice-helpers'

describe('VoiceAgent e2e', () => {
  test('loopback PC with mock voice agent TTS injection', async () => {
    const { agentOut, userInbound, cleanup } = await createVoiceLoopback()

    const agent = new VoiceAgent({
      stt: { provider: 'mock', language: 'en' },
      tts: { provider: 'mock' },
      events: { mode: 'both' },
    })

    await agent.attach({ inboundTrack: userInbound, outboundTrack: agentOut })
    await agent.start()
    await agent.sendTextToTTS('Hello loopback')
    await agent.stop()
    await cleanup()
  })
})
