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

  test('callback mode delivers speech events to on() handlers', async () => {
    const { agentOut, userInbound, cleanup } = await createVoiceLoopback()
    const agent = new VoiceAgent({ ...mockVoiceConfig, events: { mode: 'callback' } })

    await agent.attach({ inboundTrack: userInbound, outboundTrack: agentOut })
    await agent.start()

    const sawStart = new Promise<void>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error('timed out waiting for agent_speaking_start')),
        5000,
      )
      agent.on('agent_speaking_start', () => {
        clearTimeout(timer)
        resolve()
      })
    })
    await agent.sendTextToTTS('ping')
    await sawStart

    await agent.stop()
    await cleanup()
  })
})
