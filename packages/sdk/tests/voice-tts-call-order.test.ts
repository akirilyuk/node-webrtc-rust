/**
 * `sendTextToTTS` queues and plays replies in call order.
 *
 * The native pre-enqueue test hook holds each earlier call back longer than the next one, so
 * without call-order tickets the last call would reach the queue (and play) first.
 */
import { describe, expect, test } from 'vitest'

import { VoiceAgent } from '../src/voice'
import { createVoiceLoopback } from './voice-helpers'

const N = 20

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

describe('sendTextToTTS call order', () => {
  test('concurrent calls emit agent_reply_start in call order', async () => {
    const { agentOut, userInbound, cleanup } = await createVoiceLoopback()
    const agent = new VoiceAgent({
      tts: { provider: 'mock' },
      events: { mode: 'callback' },
    })
    const starts: string[] = []
    agent.on('agent_reply_start', (e) => starts.push(e.replyId ?? ''))
    await agent.attach({ inboundTrack: userInbound, outboundTrack: agentOut })
    await agent.start()
    const native = (
      agent as unknown as {
        native: { __testSetTtsPreEnqueueDelays: (delays: number[] | null) => void }
      }
    ).native
    // Call i waits (N - i) * 5 ms before it reaches the queue: reverse arrival order.
    native.__testSetTtsPreEnqueueDelays(Array.from({ length: N }, (_, i) => (N - i) * 5))

    const sends = Array.from({ length: N }, (_, i) =>
      agent.sendTextToTTS(`${i}`, { nonBlocking: true, replyId: `r${i}` }),
    )
    await Promise.all(sends)

    const deadline = Date.now() + 30_000
    while (starts.length < N && Date.now() < deadline) await sleep(20)
    expect(starts).toEqual(Array.from({ length: N }, (_, i) => `r${i}`))

    native.__testSetTtsPreEnqueueDelays(null)
    await agent.stop().catch(() => undefined)
    await cleanup()
  }, 60_000)
})
