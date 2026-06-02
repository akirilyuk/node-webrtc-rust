import { SPEECH_EVENT_TYPE } from '@node-webrtc-rust/sdk/voice'
import { describe, expect, it } from 'vitest'

import {
  evaluateBargeUtteranceFinal,
  evaluateSemanticBargeEventOrder,
  evaluateToneMustNotBarge,
  formatRecordedSpeechEvent,
  phase1BaselineComplete,
  phase2EventsComplete,
  phase3EventsComplete,
  phase3EventsTerminal,
  type RecordedSpeechEvent,
} from './roundtrip-barge-in-helpers.js'

describe('evaluateBargeUtteranceFinal', () => {
  it('passes when barge phrase is recognized with paired end and final', () => {
    const events: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
      { type: SPEECH_EVENT_TYPE.userSpeechPartial, atMs: 1400, text: 'stop' },
      { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 1450 },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 1500 },
      { type: SPEECH_EVENT_TYPE.userSpeakingEnd, atMs: 3000 },
      { type: SPEECH_EVENT_TYPE.userSpeechFinal, atMs: 3100, text: 'stop now please' },
    ]
    const result = evaluateBargeUtteranceFinal({
      events,
      expectedPhrase: 'stop now please',
    })
    expect(result.passed).toBe(true)
    expect(result.similarity).toBeGreaterThanOrEqual(0.6)
  })

  it('fails when user_speech_final is missing after barge_in', () => {
    const events: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
      { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 1450 },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 1500 },
    ]
    const result = evaluateBargeUtteranceFinal({
      events,
      expectedPhrase: 'stop now please',
    })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('user_speech_final'))).toBe(true)
  })
})

describe('roundtrip-barge-in helpers', () => {
  it('formatRecordedSpeechEvent includes offset, type, and optional text', () => {
    expect(formatRecordedSpeechEvent({ type: SPEECH_EVENT_TYPE.bargeIn, atMs: 1450 })).toBe(
      '+1450ms barge_in',
    )
    expect(
      formatRecordedSpeechEvent({
        type: SPEECH_EVENT_TYPE.userSpeechPartial,
        atMs: 1400,
        text: 'stop speaking',
      }),
    ).toBe('+1400ms user_speech_partial "stop speaking"')
  })

  it('phase completion helpers detect baseline and barge sequences', () => {
    const baseline: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 5000 },
    ]
    expect(phase1BaselineComplete(baseline)).toBe(true)
    expect(phase2EventsComplete(baseline)).toBe(true)

    const barge: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
      { type: SPEECH_EVENT_TYPE.userSpeechPartial, atMs: 1400, text: 'stop' },
      { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 1450 },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 1500 },
      { type: SPEECH_EVENT_TYPE.userSpeakingEnd, atMs: 3000 },
      { type: SPEECH_EVENT_TYPE.userSpeechFinal, atMs: 3100, text: 'stop now please' },
    ]
    expect(phase3EventsComplete(barge)).toBe(true)
    expect(phase3EventsTerminal(barge)).toBe(true)

    const bargeNoFinal: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
      { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 1450 },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 1500 },
    ]
    expect(phase3EventsComplete(bargeNoFinal)).toBe(false)
    expect(phase3EventsTerminal(bargeNoFinal)).toBe(false)

    const noBarge: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 9000 },
    ]
    expect(phase3EventsComplete(noBarge)).toBe(false)
    expect(phase3EventsTerminal(noBarge)).toBe(true)

    const userNoPartial: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
      { type: SPEECH_EVENT_TYPE.userSpeakingStart, atMs: 800 },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 9000 },
    ]
    expect(phase3EventsTerminal(userNoPartial)).toBe(true)
  })

  it('evaluateSemanticBargeEventOrder accepts partial → barge_in → agent_speaking_end', () => {
    const events: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
      { type: SPEECH_EVENT_TYPE.userSpeakingStart, atMs: 1200 },
      { type: SPEECH_EVENT_TYPE.userSpeechPartial, atMs: 1400, text: 'stop speaking' },
      { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 1450 },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 1500 },
      { type: SPEECH_EVENT_TYPE.userSpeechFinal, atMs: 3200, text: 'stop speaking' },
    ]
    const result = evaluateSemanticBargeEventOrder({ events })
    expect(result.passed).toBe(true)
    expect(result.failures).toHaveLength(0)
  })

  it('rejects barge_in before qualifying partial during agent TTS', () => {
    const events: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
      { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 200 },
      { type: SPEECH_EVENT_TYPE.userSpeakingStart, atMs: 1200 },
      { type: SPEECH_EVENT_TYPE.userSpeechPartial, atMs: 5000, text: 'stop' },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 5100 },
    ]
    const result = evaluateSemanticBargeEventOrder({ events })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('barge_in'))).toBe(true)
  })

  it('rejects agent_speaking_end before barge_in', () => {
    const events: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
      { type: SPEECH_EVENT_TYPE.userSpeechPartial, atMs: 1400, text: 'stop' },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 5000 },
      { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 5100 },
    ]
    const result = evaluateSemanticBargeEventOrder({ events })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('agent_speaking_end'))).toBe(true)
  })

  it('rejects large gap between partial and barge_in', () => {
    const events: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
      { type: SPEECH_EVENT_TYPE.userSpeechPartial, atMs: 1000, text: 'stop' },
      { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 2000 },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 2100 },
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
        { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 0 },
        { type: SPEECH_EVENT_TYPE.userSpeakingStart, atMs: 500 },
        { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 600 },
      ],
      bargeCount: 1,
    })
    expect(result.passed).toBe(false)
  })
})
