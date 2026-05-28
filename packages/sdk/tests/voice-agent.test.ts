import { describe, expect, test } from 'vitest'

import { LocalAudioTrack } from '../src'
import { VoiceAgent } from '../src/voice'

const mockConfig = {
  stt: { provider: 'mock' as const, language: 'en' },
  tts: { provider: 'mock' as const },
  events: { mode: 'callback' as const },
}

describe('VoiceAgent', () => {
  test('creates with mock vendors', () => {
    const agent = new VoiceAgent(mockConfig)
    expect(agent.getNativeAgent()).toBeDefined()
  })

  test('sendTextToTTS after attach/start', async () => {
    const agent = new VoiceAgent(mockConfig)
    const outbound = new LocalAudioTrack('agent-out', 'agent-stream')
    await agent.attach({
      inboundTrack: { readSample: async () => Buffer.alloc(3840) } as never,
      outboundTrack: outbound,
    })
    await agent.start()
    await agent.sendTextToTTS('Hello from mock TTS')
    await agent.stop()
  })

  test('registers speech callbacks', async () => {
    const agent = new VoiceAgent({ ...mockConfig, events: { mode: 'both' } })
    const events: string[] = []
    agent.on('agent_speaking_start', (event) => {
      events.push(event.type)
    })

    const outbound = new LocalAudioTrack('agent-out-2', 'agent-stream-2')
    await agent.attach({
      inboundTrack: { readSample: async () => Buffer.alloc(3840) } as never,
      outboundTrack: outbound,
    })
    await agent.start()
    await agent.sendTextToTTS('ping')
    await new Promise((resolve) => setTimeout(resolve, 100))
    await agent.stop()

    expect(events).toContain('agent_speaking_start')
  })
})
