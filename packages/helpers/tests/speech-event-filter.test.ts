import { describe, expect, it, vi } from 'vitest'

import { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'

type SpeechEventLike = { type: string; holdMode?: string; bufferedMs?: number }

function makeHost(options: Record<string, unknown>) {
  return new VoiceAgentSessionHost({ room: 'test-room', on: vi.fn() } as never, [], {
    voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } } as never,
    sessionMode: 'voice',
    ...options,
  } as never) as unknown as {
    wireSpeechEvents: (peerId: string, session: unknown) => () => void
  }
}

async function wireAndCollect(options: Record<string, unknown>, events: SpeechEventLike[]) {
  const send = vi.fn()
  const session = {
    controlChannel: { readyState: 'open', send },
    agent: {
      speechEvents: async function* () {
        for (const event of events) yield event
      },
    },
  }
  makeHost(options).wireSpeechEvents('client-1', session)
  // Let the async iteration drain.
  await new Promise((resolve) => setTimeout(resolve, 20))
  return send.mock.calls.map(([raw]) => JSON.parse(raw as string) as Record<string, unknown>)
}

const EVENTS: SpeechEventLike[] = [
  { type: 'stt_hold_started', holdMode: 'buffer_replay', bufferedMs: 100 },
  { type: 'user_speech_final' },
]

describe('VoiceAgentSessionHost speechEventFilter', () => {
  it('forwards every event when the option is absent (no voiceHandler)', async () => {
    const sent = await wireAndCollect({}, EVENTS)
    expect(sent.map((m) => m.event)).toEqual(['stt_hold_started', 'user_speech_final'])
    expect(sent[0]).toMatchObject({ holdMode: 'buffer_replay', bufferedMs: 100 })
  })

  it('forwards every event when the option is absent (with onSpeechEvent)', async () => {
    const sent = await wireAndCollect({ voiceHandler: { onSpeechEvent: vi.fn() } }, EVENTS)
    expect(sent.map((m) => m.event)).toEqual(['stt_hold_started', 'user_speech_final'])
  })

  it('drops filtered events from the wire (no voiceHandler)', async () => {
    const sent = await wireAndCollect(
      { speechEventFilter: (e: SpeechEventLike) => e.type !== 'stt_hold_started' },
      EVENTS,
    )
    expect(sent.map((m) => m.event)).toEqual(['user_speech_final'])
  })

  it('drops filtered events from the wire but still calls onSpeechEvent', async () => {
    const onSpeechEvent = vi.fn()
    const sent = await wireAndCollect(
      {
        voiceHandler: { onSpeechEvent },
        speechEventFilter: (e: SpeechEventLike) => e.type !== 'stt_hold_started',
      },
      EVENTS,
    )
    expect(sent.map((m) => m.event)).toEqual(['user_speech_final'])
    expect(onSpeechEvent).toHaveBeenCalledTimes(2)
  })
})
