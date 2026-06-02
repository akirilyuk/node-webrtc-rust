import { SPEECH_EVENT_TYPE } from '@node-webrtc-rust/sdk/voice'
import { describe, expect, it } from 'vitest'

import {
  evaluateBargePathLifecycle,
  evaluateC1NotFoundPath,
  evaluateNormalUtteranceLifecycle,
  evaluateNoPartialWithoutFinal,
  evaluateSttLifecycleOnBargePath,
  evaluateTonePhaseLifecycle,
  evaluateUtteranceSessionCloseWithFinal,
  evaluateVadSttSessionOpen,
  type LifecycleSpeechEvent,
} from './roundtrip-stt-lifecycle-helpers.js'

function ev(type: LifecycleSpeechEvent['type'], atMs: number, text?: string): LifecycleSpeechEvent {
  return { type, atMs, text }
}

describe('evaluateVadSttSessionOpen', () => {
  it('accepts vad_triggered → user_stt_start → stt_stream_start', () => {
    const events = [
      ev(SPEECH_EVENT_TYPE.vadTriggered, 100),
      ev(SPEECH_EVENT_TYPE.userSttStart, 100),
      ev(SPEECH_EVENT_TYPE.sttStreamStart, 101),
    ]
    expect(evaluateVadSttSessionOpen({ events }).passed).toBe(true)
  })

  it('requires two open sequences for two-phrase roundtrip', () => {
    const events = [
      ev(SPEECH_EVENT_TYPE.vadTriggered, 100),
      ev(SPEECH_EVENT_TYPE.userSttStart, 100),
      ev(SPEECH_EVENT_TYPE.sttStreamStart, 101),
      ev(SPEECH_EVENT_TYPE.sttStreamEnd, 5000),
      ev(SPEECH_EVENT_TYPE.userSttEnd, 5001),
      ev(SPEECH_EVENT_TYPE.vadTriggered, 8000),
      ev(SPEECH_EVENT_TYPE.userSttStart, 8000),
      ev(SPEECH_EVENT_TYPE.sttStreamStart, 8001),
    ]
    expect(evaluateVadSttSessionOpen({ events, expectCount: 2 }).passed).toBe(true)
    expect(evaluateVadSttSessionOpen({ events, expectCount: 2 }).failures).toHaveLength(0)
  })

  it('rejects stt_stream_start before user_stt_start', () => {
    const events = [
      ev(SPEECH_EVENT_TYPE.vadTriggered, 100),
      ev(SPEECH_EVENT_TYPE.sttStreamStart, 100),
      ev(SPEECH_EVENT_TYPE.userSttStart, 101),
    ]
    const result = evaluateVadSttSessionOpen({ events })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('user_stt_start'))).toBe(true)
  })
})

describe('evaluateUtteranceSessionCloseWithFinal', () => {
  it('accepts stt_stream_end → user_stt_end → user_speaking_end → user_speech_final', () => {
    const events = [
      ev(SPEECH_EVENT_TYPE.vadTriggered, 100),
      ev(SPEECH_EVENT_TYPE.sttStreamStart, 101),
      ev(SPEECH_EVENT_TYPE.userSpeechPartial, 500, 'hello'),
      ev(SPEECH_EVENT_TYPE.sttStreamEnd, 2000),
      ev(SPEECH_EVENT_TYPE.userSttEnd, 2001),
      ev(SPEECH_EVENT_TYPE.userSpeakingEnd, 2002),
      ev(SPEECH_EVENT_TYPE.userSpeechFinal, 2003, 'hello world'),
    ]
    expect(evaluateUtteranceSessionCloseWithFinal({ events }).passed).toBe(true)
  })

  it('rejects missing stt_stream_end', () => {
    const events = [
      ev(SPEECH_EVENT_TYPE.userSttEnd, 2001),
      ev(SPEECH_EVENT_TYPE.userSpeakingEnd, 2002),
      ev(SPEECH_EVENT_TYPE.userSpeechFinal, 2003, 'hello'),
    ]
    const result = evaluateUtteranceSessionCloseWithFinal({ events })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('stt_stream_end'))).toBe(true)
  })
})

describe('evaluateC1NotFoundPath', () => {
  it('accepts stt_stream_end → user_stt_not_found → user_stt_end without final', () => {
    const events = [
      ev(SPEECH_EVENT_TYPE.vadTriggered, 100),
      ev(SPEECH_EVENT_TYPE.sttStreamStart, 101),
      ev(SPEECH_EVENT_TYPE.sttStreamEnd, 4100),
      ev(SPEECH_EVENT_TYPE.userSttNotFound, 4101),
      ev(SPEECH_EVENT_TYPE.userSttEnd, 4102),
    ]
    expect(evaluateC1NotFoundPath({ events }).passed).toBe(true)
  })

  it('rejects user_speech_final with user_stt_not_found', () => {
    const events = [
      ev(SPEECH_EVENT_TYPE.sttStreamEnd, 4100),
      ev(SPEECH_EVENT_TYPE.userSttNotFound, 4101),
      ev(SPEECH_EVENT_TYPE.userSttEnd, 4102),
      ev(SPEECH_EVENT_TYPE.userSpeechFinal, 4200, 'oops'),
    ]
    const result = evaluateC1NotFoundPath({ events })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('user_speech_final'))).toBe(true)
  })
})

describe('evaluateNoPartialWithoutFinal', () => {
  it('flags orphan partials', () => {
    const events = [
      ev(SPEECH_EVENT_TYPE.userSpeechPartial, 100, 'hi'),
      ev(SPEECH_EVENT_TYPE.userSpeakingEnd, 500),
    ]
    const result = evaluateNoPartialWithoutFinal({ events })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('orphan partial'))).toBe(true)
  })

  it('passes when partial is followed by final', () => {
    const events = [
      ev(SPEECH_EVENT_TYPE.userSpeechPartial, 100, 'hi'),
      ev(SPEECH_EVENT_TYPE.userSpeechFinal, 500, 'hi there'),
    ]
    expect(evaluateNoPartialWithoutFinal({ events }).passed).toBe(true)
  })
})

describe('evaluateSttLifecycleOnBargePath', () => {
  it('accepts full barge listen open before partial', () => {
    const events = [
      ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 0),
      ev(SPEECH_EVENT_TYPE.vadTriggered, 800),
      ev(SPEECH_EVENT_TYPE.userSttStart, 800),
      ev(SPEECH_EVENT_TYPE.sttStreamStart, 801),
      ev(SPEECH_EVENT_TYPE.userSpeechPartial, 1400, 'stop'),
      ev(SPEECH_EVENT_TYPE.bargeIn, 1450),
      ev(SPEECH_EVENT_TYPE.agentSpeakingEnd, 1500),
    ]
    expect(evaluateSttLifecycleOnBargePath({ events }).passed).toBe(true)
  })

  it('rejects partial before stt_stream_start', () => {
    const events = [
      ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 0),
      ev(SPEECH_EVENT_TYPE.userSpeechPartial, 500, 'stop'),
      ev(SPEECH_EVENT_TYPE.vadTriggered, 800),
      ev(SPEECH_EVENT_TYPE.sttStreamStart, 801),
    ]
    const result = evaluateSttLifecycleOnBargePath({ events })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('stt_stream_start'))).toBe(true)
  })
})

describe('evaluateTonePhaseLifecycle', () => {
  it('rejects barge_in during tone phase', () => {
    const result = evaluateTonePhaseLifecycle({
      events: [ev(SPEECH_EVENT_TYPE.bargeIn, 500)],
      bargeCount: 1,
    })
    expect(result.passed).toBe(false)
  })

  it('accepts C1 not_found path without final', () => {
    const events = [
      ev(SPEECH_EVENT_TYPE.vadTriggered, 100),
      ev(SPEECH_EVENT_TYPE.sttStreamStart, 101),
      ev(SPEECH_EVENT_TYPE.sttStreamEnd, 4100),
      ev(SPEECH_EVENT_TYPE.userSttNotFound, 4101),
      ev(SPEECH_EVENT_TYPE.userSttEnd, 4102),
    ]
    expect(evaluateTonePhaseLifecycle({ events, bargeCount: 0, label: 'Phase 2' }).passed).toBe(
      true,
    )
  })
})

describe('composite lifecycle evaluators', () => {
  it('evaluateNormalUtteranceLifecycle merges open, close, and orphan checks', () => {
    const events = [
      ev(SPEECH_EVENT_TYPE.vadTriggered, 100),
      ev(SPEECH_EVENT_TYPE.userSttStart, 100),
      ev(SPEECH_EVENT_TYPE.sttStreamStart, 101),
      ev(SPEECH_EVENT_TYPE.userSpeechPartial, 400, 'one two'),
      ev(SPEECH_EVENT_TYPE.sttStreamEnd, 3000),
      ev(SPEECH_EVENT_TYPE.userSttEnd, 3001),
      ev(SPEECH_EVENT_TYPE.userSpeakingEnd, 3002),
      ev(SPEECH_EVENT_TYPE.userSpeechFinal, 3003, 'one two three'),
    ]
    expect(evaluateNormalUtteranceLifecycle({ events }).passed).toBe(true)
  })

  it('evaluateBargePathLifecycle requires barge-path open and session close', () => {
    const events = [
      ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 0),
      ev(SPEECH_EVENT_TYPE.vadTriggered, 800),
      ev(SPEECH_EVENT_TYPE.userSttStart, 800),
      ev(SPEECH_EVENT_TYPE.sttStreamStart, 801),
      ev(SPEECH_EVENT_TYPE.userSpeechPartial, 1400, 'stop'),
      ev(SPEECH_EVENT_TYPE.bargeIn, 1450),
      ev(SPEECH_EVENT_TYPE.agentSpeakingEnd, 1500),
      ev(SPEECH_EVENT_TYPE.sttStreamEnd, 3000),
      ev(SPEECH_EVENT_TYPE.userSttEnd, 3001),
      ev(SPEECH_EVENT_TYPE.userSpeakingEnd, 3002),
      ev(SPEECH_EVENT_TYPE.userSpeechFinal, 3003, 'stop now'),
    ]
    expect(evaluateBargePathLifecycle({ events }).passed).toBe(true)
  })
})
