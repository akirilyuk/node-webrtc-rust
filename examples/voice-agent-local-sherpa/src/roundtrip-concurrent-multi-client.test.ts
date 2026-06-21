import { describe, expect, it } from 'vitest'

import {
  evaluateConcurrentRoundtrip,
  evaluateConcurrentWindow,
  finalContainsKeyword,
} from './roundtrip-concurrent-timing-helpers.js'

describe('roundtrip-concurrent-timing-helpers', () => {
  it('evaluateConcurrentWindow passes when spread is within limit', () => {
    const result = evaluateConcurrentWindow([100, 120, 150], 500)
    expect(result.passed).toBe(true)
    expect(result.spreadMs).toBe(50)
  })

  it('evaluateConcurrentWindow fails when spread exceeds limit', () => {
    const result = evaluateConcurrentWindow([100, 800], 500)
    expect(result.passed).toBe(false)
    expect(result.failures[0]).toMatch(/spread/)
  })

  it('finalContainsKeyword matches normalized text', () => {
    expect(finalContainsKeyword('I heard alpha today', 'alpha')).toBe(true)
    expect(finalContainsKeyword('bravo team', 'alpha')).toBe(false)
  })

  it('evaluateConcurrentRoundtrip checks enqueue and keyword routing', () => {
    const evaluation = evaluateConcurrentRoundtrip({
      legs: [
        {
          legId: 'tab1',
          phrase: 'alpha one',
          keyword: 'alpha',
          agentSpeakingStartMs: 10,
          finalText: 'alpha one',
          finalMs: 500,
        },
        {
          legId: 'tab2',
          phrase: 'bravo two',
          keyword: 'bravo',
          agentSpeakingStartMs: 40,
          finalText: 'bravo two',
          finalMs: 520,
        },
        {
          legId: 'tab3',
          phrase: 'charlie three',
          keyword: 'charlie',
          agentSpeakingStartMs: 55,
          finalText: 'charlie three',
          finalMs: 540,
        },
      ],
      enqueueMs: 50,
      maxEnqueueMs: 200,
      maxAgentStartSpreadMs: 500,
      maxFinalSpreadMs: 500,
    })
    expect(evaluation.passed).toBe(true)
  })
})
