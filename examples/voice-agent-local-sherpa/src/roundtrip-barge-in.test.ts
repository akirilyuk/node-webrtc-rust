import { SPEECH_EVENT_TYPE } from '@node-webrtc-rust/sdk/voice'
import { describe, expect, it } from 'vitest'

import {
  evaluateBargeUtteranceFinal,
  evaluateSemanticBargeEventOrder,
  evaluateSttLifecycleOnBargePath,
  evaluateNoPartialWithoutFinal,
  evaluateToneMustNotBarge,
  formatRecordedSpeechEvent,
  phraseLeadWord,
  phase1BaselineComplete,
  phase2EventsComplete,
  phase3BargeObserved,
  phase3EventsComplete,
  phase3EventsTerminal,
  textContainsWholeWord,
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
    expect(result.similarity).toBeGreaterThanOrEqual(0.75)
  })

  it('fails when lead word is missing from final (main regression: "Now please" vs "stop now please")', () => {
    const events: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 439 },
      { type: SPEECH_EVENT_TYPE.userSpeechPartial, atMs: 2509, text: 'Now' },
      { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 2509 },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 2509 },
      { type: SPEECH_EVENT_TYPE.userSpeakingEnd, atMs: 56495 },
      { type: SPEECH_EVENT_TYPE.userSpeechFinal, atMs: 56495, text: 'Now please' },
    ]
    const result = evaluateBargeUtteranceFinal({
      events,
      expectedPhrase: 'stop now please',
    })
    expect(result.passed).toBe(false)
    expect(result.similarity).toBeCloseTo(2 / 3, 5)
    expect(result.failures.some((f) => f.includes('lead word "stop"'))).toBe(true)
    expect(result.failures.some((f) => f.includes('similarity'))).toBe(true)
  })

  it('fails when barge-trigger partial omits lead word', () => {
    const events: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 439 },
      { type: SPEECH_EVENT_TYPE.userSpeechPartial, atMs: 2509, text: 'Now' },
      { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 2509 },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 2509 },
    ]
    const result = evaluateSemanticBargeEventOrder({
      events,
      expectedPhrase: 'stop now please',
      label: 'Phase 3',
    })
    expect(result.passed).toBe(false)
    expect(
      result.failures.some((f) => f.includes('barge-trigger partial missing lead word "stop"')),
    ).toBe(true)
  })

  it('phraseLeadWord and textContainsWholeWord helpers', () => {
    expect(phraseLeadWord('stop now please')).toBe('stop')
    expect(textContainsWholeWord('Now please', 'stop')).toBe(false)
    expect(textContainsWholeWord('stop now', 'stop')).toBe(true)
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
    expect(phase3BargeObserved(bargeNoFinal)).toBe(true)
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

  it('evaluateSttLifecycleOnBargePath requires vad → user_stt → stt_stream before partial', () => {
    const events: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
      { type: SPEECH_EVENT_TYPE.vadTriggered, atMs: 1200 },
      { type: SPEECH_EVENT_TYPE.userSttStart, atMs: 1200 },
      { type: SPEECH_EVENT_TYPE.sttStreamStart, atMs: 1201 },
      { type: SPEECH_EVENT_TYPE.userSpeechPartial, atMs: 1400, text: 'stop speaking' },
      { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 1450 },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 1500 },
    ]
    expect(evaluateSttLifecycleOnBargePath({ events }).passed).toBe(true)
  })

  it('evaluateNoPartialWithoutFinal rejects orphan partial after barge', () => {
    const events: RecordedSpeechEvent[] = [
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
      { type: SPEECH_EVENT_TYPE.userSpeechPartial, atMs: 1400, text: 'stop' },
      { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 1450 },
      { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 1500 },
    ]
    const result = evaluateNoPartialWithoutFinal({ events, label: 'Phase 3' })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('orphan partial'))).toBe(true)
  })
})
