import { describe, expect, it } from 'vitest'

import {
  DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS,
  evaluateFinalSequence,
  type FinalEventRecord,
} from './roundtrip-counting.js'

describe('evaluateFinalSequence (two-phrase)', () => {
  const okPair = (text: string, gapMs: number): FinalEventRecord => ({
    text,
    speakingEndAtMs: 1000,
    finalAtMs: 1000 + gapMs,
    gapMs,
  })

  it('passes for two finals with tight end→final gaps', () => {
    const result = evaluateFinalSequence({
      records: [
        okPair('one two three four five six seven eight nine ten', 50),
        okPair('I am done speaking', 100),
      ],
      expectedCount: 2,
      maxGapMs: DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS,
      minNumberWordsFirst: 8,
      textIncludes: ['', 'done'],
    })
    expect(result.passed).toBe(true)
  })

  it('fails when only one final (merged / dropped first turn)', () => {
    const result = evaluateFinalSequence({
      records: [okPair('one two ten I am done speaking', 0)],
      expectedCount: 2,
      textIncludes: ['', 'done'],
    })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('expected 2'))).toBe(true)
  })

  it('fails on orphan speaking_end gap (final seconds later)', () => {
    const result = evaluateFinalSequence({
      records: [okPair('hello', 8000)],
      expectedCount: 1,
      maxGapMs: 500,
    })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('gap'))).toBe(true)
  })
})
