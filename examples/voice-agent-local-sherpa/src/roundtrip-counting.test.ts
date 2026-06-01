import { describe, expect, it } from 'vitest'

import {
  countNumberWordsInTranscript,
  DEFAULT_COUNTING_PHRASE,
  evaluateCountingRoundtrip,
  NUMBER_WORDS_ONE_TO_TWENTY,
} from './roundtrip-counting.js'

describe('roundtrip-counting helpers', () => {
  it('DEFAULT_COUNTING_PHRASE lists one through twenty', () => {
    const words = DEFAULT_COUNTING_PHRASE.split(/\s+/)
    expect(words).toHaveLength(20)
    expect(words).toEqual([...NUMBER_WORDS_ONE_TO_TWENTY])
  })

  it('countNumberWordsInTranscript finds embedded number words', () => {
    const text = 'heard one two three four five six seven eight nine ten and more'
    expect(countNumberWordsInTranscript(text)).toBe(10)
  })

  it('evaluateCountingRoundtrip passes on single final with enough numbers', () => {
    const result = evaluateCountingRoundtrip({
      phrase: DEFAULT_COUNTING_PHRASE,
      recognized:
        'one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty',
      stats: {
        finals: [
          'one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty',
        ],
        speakingEndCount: 1,
        speakingStartCount: 1,
        partialCount: 12,
        bargeInCount: 0,
        speakingEndAtMs: null,
        speechFinalAtMs: null,
      },
      minNumberWords: 16,
    })
    expect(result.passed).toBe(true)
    expect(result.failures).toHaveLength(0)
    expect(result.numberWordsFound).toBeGreaterThanOrEqual(16)
  })

  it('evaluateCountingRoundtrip fails on multiple finals (scattered utterances)', () => {
    const result = evaluateCountingRoundtrip({
      phrase: DEFAULT_COUNTING_PHRASE,
      recognized: 'one two three four five six seven eight',
      stats: {
        finals: ['one two three four', 'five six seven eight'],
        speakingEndCount: 2,
        speakingStartCount: 2,
        partialCount: 4,
        bargeInCount: 0,
        speakingEndAtMs: null,
        speechFinalAtMs: null,
      },
    })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('user_speech_final'))).toBe(true)
    expect(result.failures.some((f) => f.includes('user_speaking_end'))).toBe(true)
  })
})
