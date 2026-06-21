import { SPEECH_EVENT_TYPE } from '@node-webrtc-rust/sdk/voice'
import { describe, expect, it } from 'vitest'

import { DEFAULT_COUNTING_PHRASE_ONE_TO_TEN } from './roundtrip-counting.js'
import { formatAgent2EchoReply } from './roundtrip-counting-echo.js'
import { evaluateBargeUtteranceFinal } from './roundtrip-barge-in-helpers.js'
import {
  DEFAULT_BARGE_PHRASE,
  DEFAULT_RECOVERY_PHRASE,
  evaluateInterruptedEchoLeg,
} from './roundtrip-counting-barge-recovery.js'

describe('roundtrip-counting-barge-recovery helpers', () => {
  it('DEFAULT_RECOVERY_PHRASE is non-empty', () => {
    expect(DEFAULT_RECOVERY_PHRASE.length).toBeGreaterThan(10)
  })

  it('DEFAULT_BARGE_PHRASE matches semantic barge-in default', () => {
    expect(DEFAULT_BARGE_PHRASE).toBe('stop now please')
  })

  it('evaluateInterruptedEchoLeg fails when six number words exceed similarity cap', () => {
    const echoText = formatAgent2EchoReply('one two three four five six seven eight nine ten')
    const result = evaluateInterruptedEchoLeg({
      echoText,
      recognized: 'you said one two three four five six',
      maxNumberWords: 6,
      maxSimilarity: 0.55,
    })
    expect(result.passed).toBe(false)
    expect(result.similarity).toBeGreaterThan(0.55)
  })

  it('evaluateInterruptedEchoLeg passes when transcript is a short tail', () => {
    const echoText = formatAgent2EchoReply('one two three four five six seven eight nine ten')
    const result = evaluateInterruptedEchoLeg({
      echoText,
      recognized: 'you said one two three four',
      maxNumberWords: 6,
      maxSimilarity: 0.55,
    })
    expect(result.passed).toBe(true)
    expect(result.numberWordsFound).toBeLessThanOrEqual(6)
  })

  it('evaluateInterruptedEchoLeg fails when full echo is heard', () => {
    const heard = 'one two three four five six seven eight nine ten'
    const echoText = formatAgent2EchoReply(heard)
    const result = evaluateInterruptedEchoLeg({
      echoText,
      recognized: `you said ${heard}`,
      maxNumberWords: 6,
      maxSimilarity: 0.55,
    })
    expect(result.passed).toBe(false)
    expect(result.failures.length).toBeGreaterThan(0)
  })

  it('evaluateInterruptedEchoLeg fails on empty transcript', () => {
    const echoText = formatAgent2EchoReply(DEFAULT_COUNTING_PHRASE_ONE_TO_TEN)
    const result = evaluateInterruptedEchoLeg({
      echoText,
      recognized: '',
    })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('empty'))).toBe(true)
  })

  it('Round 2 barge phrase check passes when Agent2 final matches injected phrase', () => {
    const result = evaluateBargeUtteranceFinal({
      events: [
        { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
        { type: SPEECH_EVENT_TYPE.userSpeechPartial, atMs: 900, text: 'stop now' },
        { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 950 },
        { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 1000 },
        { type: SPEECH_EVENT_TYPE.userSpeakingEnd, atMs: 2200 },
        { type: SPEECH_EVENT_TYPE.userSpeechFinal, atMs: 2300, text: DEFAULT_BARGE_PHRASE },
      ],
      expectedPhrase: DEFAULT_BARGE_PHRASE,
      label: 'Round 2 barge',
    })
    expect(result.passed).toBe(true)
  })
})
