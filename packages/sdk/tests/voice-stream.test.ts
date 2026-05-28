import { describe, expect, test } from 'vitest'

import { VoiceAgent } from '../src/voice'
import type { SpeechEvent } from '../src/voice/types'
import { createVoiceLoopback, mockVoiceConfig } from './voice-helpers'

describe('VoiceAgent speechEvents stream', () => {
  test('pullSpeechEvent yields agent speaking events', async () => {
    const { agentOut, userInbound, cleanup } = await createVoiceLoopback()
    const agent = new VoiceAgent({ ...mockVoiceConfig, events: { mode: 'stream' } })

    await agent.attach({ inboundTrack: userInbound, outboundTrack: agentOut })
    await agent.start()

    const collected: SpeechEvent[] = []
    const stream = agent.speechEvents()
    const pump = (async () => {
      for await (const event of stream) {
        collected.push(event)
        if (collected.length >= 1) break
      }
    })()

    await agent.sendTextToTTS('stream test')
    await Promise.race([pump, new Promise((resolve) => setTimeout(resolve, 1000))])
    await agent.stop()
    await cleanup()

    expect(
      collected.some((e) => e.type === 'agent_speaking_start' || e.type === 'agent_speaking_end'),
    ).toBe(true)
  })
})
