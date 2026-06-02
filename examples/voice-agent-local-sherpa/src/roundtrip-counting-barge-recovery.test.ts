import { describe, expect, it } from 'vitest'

import { DEFAULT_COUNTING_PHRASE_ONE_TO_TEN } from './roundtrip-counting.js'
import { formatAgent2EchoReply } from './roundtrip-counting-echo.js'
import {
  DEFAULT_RECOVERY_PHRASE,
  evaluateInterruptedEchoLeg,
} from './roundtrip-counting-barge-recovery.js'

describe('roundtrip-counting-barge-recovery helpers', () => {
  it('DEFAULT_RECOVERY_PHRASE is non-empty', () => {
    expect(DEFAULT_RECOVERY_PHRASE.length).toBeGreaterThan(10)
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
})
