import { describe, expect, test } from 'vitest'

import { SPEECH_EVENT_TYPE, VoiceAgent, type SpeechEvent } from '../src/voice'
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

  test('setSttEnabled toggles native STT flag', async () => {
    const agent = new VoiceAgent(mockVoiceConfig)
    expect(await agent.sttEnabled()).toBe(true)

    await agent.setSttEnabled(false)
    expect(await agent.sttEnabled()).toBe(false)

    await agent.setSttEnabled(true)
    expect(await agent.sttEnabled()).toBe(true)
  })

  test('STT hold: begin/release/cancel lifecycle and events', async () => {
    const { agentOut, userInbound, cleanup } = await createVoiceLoopback()
    const agent = new VoiceAgent({
      ...mockVoiceConfig,
      events: { mode: 'callback' },
      replay: { maxAgeMs: 30_000 },
    })
    const events: SpeechEvent[] = []
    agent.on('speech', (event) => events.push(event))

    await expect(agent.releaseSttHold({ replay: true })).rejects.toThrow(/no STT hold/)
    await expect(agent.beginSttHold({ mode: 'buffer_replay' })).rejects.toThrow(/not running/)

    await agent.attach({ inboundTrack: userInbound, outboundTrack: agentOut })
    await agent.start()

    await agent.beginSttHold({ mode: 'first_utterance', maxBufferMs: 5000 })
    await expect(agent.beginSttHold({ mode: 'buffer_replay' })).rejects.toThrow(/already active/)
    await expect(agent.replayLastUtterance()).rejects.toThrow(/STT hold/)
    await agent.cancelSttHold()

    await agent.beginSttHold({ mode: 'buffer_replay' })
    await agent.releaseSttHold({ replay: false })
    await new Promise((resolve) => setTimeout(resolve, 50))

    const started = events.filter((e) => e.type === SPEECH_EVENT_TYPE.sttHoldStarted)
    const ended = events.filter((e) => e.type === SPEECH_EVENT_TYPE.sttHoldEnded)
    expect(started.map((e) => e.holdMode)).toEqual(['first_utterance', 'buffer_replay'])
    expect(ended.map((e) => e.holdOutcome)).toEqual(['cancelled', 'released_drop'])
    // Live loopback PCM may land between begin and release, so the held amount is not
    // deterministic; it must be a whole number of 20 ms frames and nothing is dropped.
    expect(ended[1]?.bufferedMs).toBeGreaterThanOrEqual(0)
    expect((ended[1]?.bufferedMs ?? 1) % 20).toBe(0)
    expect(ended[1]?.droppedMs).toBe(0)

    await agent.stop()
    await cleanup()
  })
})
