import { describe, expect, test } from 'vitest'

import { VoiceAgent } from '../src/voice'
import { createVoiceLoopback, mockVoiceConfig } from './voice-helpers'

describe('VoiceAgent', () => {
  test('creates with mock vendors', () => {
    const agent = new VoiceAgent(mockVoiceConfig)
    expect(agent.getNativeAgent()).toBeDefined()
  })

  test('sendTextToTTS after attach/start', async () => {
    const { agentOut, userInbound, cleanup } = await createVoiceLoopback()
    const agent = new VoiceAgent(mockVoiceConfig)

    await agent.attach({ inboundTrack: userInbound, outboundTrack: agentOut })
    await agent.start()
    await agent.sendTextToTTS('Hello from mock TTS')
    await agent.stop()
    await cleanup()
  })

  test('registers speech callbacks', async () => {
    const { agentOut, userInbound, cleanup } = await createVoiceLoopback()
    const agent = new VoiceAgent({ ...mockVoiceConfig, events: { mode: 'both' } })

    await agent.attach({ inboundTrack: userInbound, outboundTrack: agentOut })
    await agent.start()

    const sawStart = new Promise<void>((resolve) => {
      agent.on('agent_speaking_start', () => resolve())
    })
    await agent.sendTextToTTS('ping')
    await Promise.race([sawStart, new Promise((resolve) => setTimeout(resolve, 2000))])

    await agent.stop()
    await cleanup()
  })
})
