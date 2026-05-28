import { describe, expect, test, vi } from 'vitest'

import type { RTCDataChannel } from '../src/RTCDataChannel'
import { VoiceAgent, type SpeechEvent } from '../src/voice'
import {
  forwardVoiceAgentSpeechToDataChannel,
  parseVoiceControlClientMessage,
  speechEventToControlMessage,
  wireVoiceAgentToDataChannel,
} from '../src/voice/speech-event-bridge'

describe('voice control DataChannel bridge', () => {
  test('speechEventToControlMessage maps event fields', () => {
    expect(
      speechEventToControlMessage({ type: 'user_speech_final', text: 'hello' }),
    ).toEqual({
      type: 'speech_event',
      event: 'user_speech_final',
      text: 'hello',
    })
  })

  test('parseVoiceControlClientMessage accepts speak payloads', () => {
    expect(parseVoiceControlClientMessage(JSON.stringify({ type: 'speak', text: 'Hi' }))).toEqual({
      type: 'speak',
      text: 'Hi',
    })
    expect(parseVoiceControlClientMessage('not json')).toBeNull()
    expect(parseVoiceControlClientMessage(JSON.stringify({ type: 'speak', text: '' }))).toBeNull()
  })

  test('forwardVoiceAgentSpeechToDataChannel sends when channel is open', () => {
    const sent: string[] = []
    const channel = {
      readyState: 'open',
      onmessage: null as ((event: { data: string }) => void) | null,
      send: (payload: string) => {
        sent.push(payload)
      },
    } as unknown as RTCDataChannel

    const agent = {
      async *speechEvents() {
        yield { type: 'user_speech_final', text: 'testing' } as SpeechEvent
      },
    } as unknown as VoiceAgent

    const unforward = forwardVoiceAgentSpeechToDataChannel(agent, channel)

    return new Promise<void>((resolve, reject) => {
      setTimeout(() => {
        try {
          expect(sent).toHaveLength(1)
          expect(JSON.parse(sent[0]!)).toMatchObject({
            type: 'speech_event',
            event: 'user_speech_final',
            text: 'testing',
          })
          unforward()
          resolve()
        } catch (error) {
          reject(error)
        }
      }, 20)
    })
  })

  test('wireVoiceAgentToDataChannel invokes onSpeak for inbound speak messages', () => {
    const onSpeak = vi.fn()
    const channel = {
      readyState: 'open',
      onmessage: null as ((event: { data: string }) => void) | null,
      send: vi.fn(),
    } as unknown as RTCDataChannel

    const agent = new VoiceAgent({ stt: { provider: 'mock' }, tts: { provider: 'mock' } })
    wireVoiceAgentToDataChannel(agent, channel, { onSpeak })

    channel.onmessage?.({ data: JSON.stringify({ type: 'speak', text: 'Say this' }) })
    expect(onSpeak).toHaveBeenCalledWith('Say this')
  })
})
