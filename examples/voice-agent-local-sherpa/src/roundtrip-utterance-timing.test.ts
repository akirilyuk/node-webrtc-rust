import { describe, expect, it } from 'vitest'

import {
  DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS,
  evaluateSpeakingEndFinalTiming,
  type UtteranceEventStats,
} from './roundtrip-counting.js'

function stats(overrides: Partial<UtteranceEventStats>): UtteranceEventStats {
  return {
    finals: [],
    speakingEndCount: 0,
    speakingStartCount: 0,
    partialCount: 0,
    bargeInCount: 0,
    agentSpeakingStartCount: 0,
    agentSpeakingEndCount: 0,
    speakingEndAtMs: null,
    speechFinalAtMs: null,
    ...overrides,
  }
}

describe('evaluateSpeakingEndFinalTiming', () => {
  it('passes when end precedes final within max gap', () => {
    const result = evaluateSpeakingEndFinalTiming({
      stats: stats({
        finals: ['hello'],
        speakingEndCount: 1,
        speakingEndAtMs: 1000,
        speechFinalAtMs: 1200,
      }),
      maxGapMs: 500,
    })
    expect(result.passed).toBe(true)
    expect(result.gapMs).toBe(200)
  })

  it('fails when gap exceeds threshold', () => {
    const result = evaluateSpeakingEndFinalTiming({
      stats: stats({
        finals: ['hello'],
        speakingEndCount: 1,
        speakingEndAtMs: 1000,
        speechFinalAtMs: 9000,
      }),
      maxGapMs: DEFAULT_MAX_SPEAKING_END_TO_FINAL_MS,
    })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('gap'))).toBe(true)
  })

  it('fails when final arrives before speaking_end', () => {
    const result = evaluateSpeakingEndFinalTiming({
      stats: stats({
        finals: ['hello'],
        speakingEndCount: 1,
        speakingEndAtMs: 5000,
        speechFinalAtMs: 1000,
      }),
    })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('before'))).toBe(true)
  })
})
