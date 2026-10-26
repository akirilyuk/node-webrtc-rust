import { describe, expect, it } from 'vitest'

import {
  evaluateConcurrentRoundtrip,
  evaluateConcurrentWindow,
  evaluateRelativeSpread,
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

  it('evaluateRelativeSpread passes concurrent legs on a fast and on a slow host', () => {
    // Same shape (legs within a fraction of their own latency), 10x apart in speed.
    expect(evaluateRelativeSpread([1000, 1100, 1250], 0).passed).toBe(true)
    expect(evaluateRelativeSpread([10_000, 11_000, 12_500], 0).passed).toBe(true)
  })

  it('evaluateRelativeSpread fails legs that ran one after another', () => {
    // Serialized: latencies L, 2L, 3L. The spread is 2x the fastest leg's latency.
    const result = evaluateRelativeSpread([1000, 2000, 3000], 0)
    expect(result.passed).toBe(false)
    expect(result.failures[0]).toMatch(/serialized/)
    expect(evaluateRelativeSpread([13_000, 26_000, 39_000], 0).passed).toBe(false)
  })

  it('the old 518 ms absolute window failed a concurrent run that the relative oracle accepts', () => {
    // CI 2026-10-12: starts at 12904..13422 ms after the page started; requested at 500 ms.
    expect(evaluateConcurrentWindow([12_904, 13_153, 13_422], 500).passed).toBe(false)
    expect(evaluateRelativeSpread([12_904, 13_153, 13_422], 500).passed).toBe(true)
  })

  it('finalContainsKeyword matches normalized text', () => {
    expect(finalContainsKeyword('I heard alpha today', 'alpha')).toBe(true)
    expect(finalContainsKeyword('bravo team', 'alpha')).toBe(false)
  })

  it('finalContainsKeyword accepts only the four/for homophone, no misrecognition aliases', () => {
    expect(finalContainsKeyword('bravo for five six', 'four')).toBe(true)
    expect(finalContainsKeyword('Belta seven eight nine', 'delta')).toBe(false)
    expect(finalContainsKeyword('Victor seven eight nine', 'victor')).toBe(true)
  })

  it('evaluateConcurrentRoundtrip checks enqueue and keyword routing', () => {
    const evaluation = evaluateConcurrentRoundtrip({
      legs: [
        {
          legId: 'tab1',
          phrase: 'alpha one',
          keyword: 'alpha',
          agentSpeakingStartMs: 1000,
          finalText: 'alpha one',
          finalMs: 5000,
        },
        {
          legId: 'tab2',
          phrase: 'bravo two',
          keyword: 'bravo',
          agentSpeakingStartMs: 1040,
          finalText: 'bravo two',
          finalMs: 5020,
        },
        {
          legId: 'tab3',
          phrase: 'charlie three',
          keyword: 'charlie',
          agentSpeakingStartMs: 1055,
          finalText: 'charlie three',
          finalMs: 5040,
        },
      ],
      enqueueMs: 50,
      maxEnqueueMs: 200,
      startedAtMs: 0,
    })
    expect(evaluation.passed).toBe(true)
  })

  it('finalContainsKeyword accepts the for homophone', () => {
    expect(finalContainsKeyword('bravo for five six', 'four')).toBe(true)
    expect(finalContainsKeyword('bravo five six', 'four')).toBe(false)
  })
})
