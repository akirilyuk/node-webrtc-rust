import { JsSpeechEventType } from '@node-webrtc-rust/bindings'
import { describe, expect, test, vi } from 'vitest'

import { VoiceAgent, fromJsSpeechEvent } from '../src/voice/VoiceAgent'
import { SPEECH_EVENT_TYPE } from '../src/voice/types'

describe('replyId', () => {
  test('sendTextToTTS passes replyId to the native call', async () => {
    const agent = new VoiceAgent()
    const native = (
      agent as unknown as { native: { sendTextToTts: (...args: unknown[]) => unknown } }
    ).native
    const spy = vi.spyOn(native, 'sendTextToTts').mockResolvedValue(undefined)

    await agent.sendTextToTTS('x', { replyId: 'r1', nonBlocking: true })
    expect(spy).toHaveBeenLastCalledWith('x', true, undefined, 'r1')

    await agent.sendTextToTTS('y')
    expect(spy).toHaveBeenLastCalledWith('y', undefined, undefined, undefined)
  })

  test('reply events map replyId, playedMs and interrupted', () => {
    expect(SPEECH_EVENT_TYPE.agentReplyStart).toBe('agent_reply_start')
    expect(SPEECH_EVENT_TYPE.agentReplyEnd).toBe('agent_reply_end')

    const start = fromJsSpeechEvent({
      eventType: JsSpeechEventType.AgentReplyStart,
      replyId: 'r1',
      firstChunkMs: 120,
    })
    expect(start.type).toBe('agent_reply_start')
    expect(start.replyId).toBe('r1')
    expect(start.firstChunkMs).toBe(120)

    const end = fromJsSpeechEvent({
      eventType: JsSpeechEventType.AgentReplyEnd,
      replyId: 'r1',
      playedMs: 900,
      interrupted: true,
    })
    expect(end.type).toBe('agent_reply_end')
    expect(end.playedMs).toBe(900)
    expect(end.interrupted).toBe(true)
  })
})
