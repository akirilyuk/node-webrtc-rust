import { describe, expect, it } from 'vitest'

import {
  evaluateSemanticBargeEventOrder,
  evaluateToneMustNotBarge,
  type RecordedSpeechEvent,
} from './roundtrip-barge-in-helpers.js'

describe('roundtrip-barge-in helpers', () => {
  it('evaluateSemanticBargeEventOrder accepts partial → barge_in → agent_speaking_end', () => {
    const events: RecordedSpeechEvent[] = [
      { type: 'agent_speaking_start', atMs: 100 },
      { type: 'user_speaking_start', atMs: 1200 },
      { type: 'user_speech_partial', atMs: 1400, text: 'stop speaking' },
      { type: 'barge_in', atMs: 1450 },
      { type: 'agent_speaking_end', atMs: 1500 },
      { type: 'user_speech_final', atMs: 3200, text: 'stop speaking' },
    ]
    const result = evaluateSemanticBargeEventOrder({ events })
    expect(result.passed).toBe(true)
    expect(result.failures).toHaveLength(0)
  })

  it('rejects barge_in before qualifying partial during agent TTS', () => {
    const events: RecordedSpeechEvent[] = [
      { type: 'agent_speaking_start', atMs: 100 },
      { type: 'barge_in', atMs: 200 },
      { type: 'user_speaking_start', atMs: 1200 },
      { type: 'user_speech_partial', atMs: 5000, text: 'stop' },
      { type: 'agent_speaking_end', atMs: 5100 },
    ]
    const result = evaluateSemanticBargeEventOrder({ events })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('barge_in'))).toBe(true)
  })

  it('rejects agent_speaking_end before barge_in', () => {
    const events: RecordedSpeechEvent[] = [
      { type: 'agent_speaking_start', atMs: 100 },
      { type: 'user_speech_partial', atMs: 1400, text: 'stop' },
      { type: 'agent_speaking_end', atMs: 5000 },
      { type: 'barge_in', atMs: 5100 },
    ]
    const result = evaluateSemanticBargeEventOrder({ events })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('agent_speaking_end'))).toBe(true)
  })

  it('rejects large gap between partial and barge_in', () => {
    const events: RecordedSpeechEvent[] = [
      { type: 'agent_speaking_start', atMs: 100 },
      { type: 'user_speech_partial', atMs: 1000, text: 'stop' },
      { type: 'barge_in', atMs: 2000 },
      { type: 'agent_speaking_end', atMs: 2100 },
    ]
    const result = evaluateSemanticBargeEventOrder({
      events,
      maxPartialToBargeMs: 500,
    })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('partial → barge_in'))).toBe(true)
  })

  it('evaluateToneMustNotBarge rejects any barge_in', () => {
    const result = evaluateToneMustNotBarge({
      events: [
        { type: 'agent_speaking_start', atMs: 0 },
        { type: 'user_speaking_start', atMs: 500 },
        { type: 'barge_in', atMs: 600 },
      ],
      bargeCount: 1,
    })
    expect(result.passed).toBe(false)
  })
})
