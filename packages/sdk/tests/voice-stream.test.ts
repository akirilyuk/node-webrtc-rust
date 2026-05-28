import { describe, expect, test } from 'vitest'

import { LocalAudioTrack } from '../src'
import { VoiceAgent } from '../src/voice'
import type { SpeechEvent } from '../src/voice/types'

const mockConfig = {
  stt: { provider: 'mock' as const, language: 'en' },
  tts: { provider: 'mock' as const },
  events: { mode: 'stream' as const },
}

describe('VoiceAgent speechEvents stream', () => {
  test('pullSpeechEvent yields agent speaking events', async () => {
    const agent = new VoiceAgent(mockConfig)
    const outbound = new LocalAudioTrack('stream-out', 'stream-1')
    await agent.attach({
      inboundTrack: { readSample: async () => Buffer.alloc(3840) } as never,
      outboundTrack: outbound,
    })
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
    await Promise.race([pump, new Promise((resolve) => setTimeout(resolve, 500))])
    await agent.stop()

    expect(collected.some((e) => e.type === 'agent_speaking_start' || e.type === 'agent_speaking_end')).toBe(
      true,
    )
  })
})
