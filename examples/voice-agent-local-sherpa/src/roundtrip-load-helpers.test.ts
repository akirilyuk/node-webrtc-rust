import { describe, expect, it } from 'vitest'

import {
  capacityStepPasses,
  evaluateLoadRun,
  nextCapacityStep,
  percentile,
  type LoadRunEvaluation,
  type LoadTurnResult,
} from './roundtrip-load-helpers.js'

function turn(overrides: Partial<LoadTurnResult> = {}): LoadTurnResult {
  return {
    legId: 'leg1',
    turn: 1,
    keyword: 'alpha',
    finalText: 'alpha one two three',
    finalLatencyMs: 1000,
    ttsStartLatencyMs: 200,
    ...overrides,
  }
}

function evaluation(overrides: Partial<LoadRunEvaluation> = {}): LoadRunEvaluation {
  return {
    failures: [],
    finalLatencyMs: { p50: 1000, p95: 2000 },
    ttsStartLatencyMs: { p50: 100, p95: 200 },
    turnSuccessRate: 1,
    ...overrides,
  }
}

describe('percentile', () => {
  it('returns null for an empty input', () => {
    expect(percentile([], 50)).toBeNull()
  })

  it('returns the only value for a single element', () => {
    expect(percentile([5], 50)).toBe(5)
    expect(percentile([5], 95)).toBe(5)
  })

  it('uses nearest rank on 1..100', () => {
    const values = Array.from({ length: 100 }, (_, i) => i + 1)
    expect(percentile(values, 50)).toBe(50)
    expect(percentile(values, 95)).toBe(95)
  })

  it('does not depend on input order', () => {
    expect(percentile([30, 10, 20], 50)).toBe(20)
  })
})

describe('evaluateLoadRun', () => {
  it('reports a missing final, a wrong keyword and an error alongside one pass', () => {
    const result = evaluateLoadRun([
      turn({ legId: 'leg1', turn: 1 }),
      turn({ legId: 'leg2', turn: 1, finalText: null, finalLatencyMs: null }),
      turn({ legId: 'leg3', turn: 2, finalText: 'something else entirely' }),
      turn({ legId: 'leg4', turn: 3, error: 'boom', finalText: null, finalLatencyMs: null }),
    ])
    expect(result.failures).toHaveLength(3)
    expect(result.failures[0]).toMatch(/^leg2 turn 1: /)
    expect(result.failures[1]).toMatch(/^leg3 turn 2: .*missing keyword "alpha"/)
    expect(result.failures[2]).toMatch(/^leg4 turn 3: .*boom/)
    expect(result.turnSuccessRate).toBe(0.25)
  })

  it('passes every turn that has the keyword', () => {
    const result = evaluateLoadRun([turn(), turn({ turn: 2 })])
    expect(result.failures).toEqual([])
    expect(result.turnSuccessRate).toBe(1)
  })

  it('ignores null latencies in the percentile stats', () => {
    const result = evaluateLoadRun([
      turn({ finalLatencyMs: 1000, ttsStartLatencyMs: 100 }),
      turn({ turn: 2, finalLatencyMs: null, ttsStartLatencyMs: null }),
      turn({ turn: 3, finalLatencyMs: 3000, ttsStartLatencyMs: 300 }),
    ])
    expect(result.finalLatencyMs).toEqual({ p50: 1000, p95: 3000 })
    expect(result.ttsStartLatencyMs).toEqual({ p50: 100, p95: 300 })
  })

  it('yields null latency stats when no turn has a latency', () => {
    const result = evaluateLoadRun([turn({ finalLatencyMs: null, ttsStartLatencyMs: null })])
    expect(result.finalLatencyMs).toEqual({ p50: null, p95: null })
  })
})

describe('nextCapacityStep', () => {
  it('walks 2, 4, 6, 8, 10, 12, 16, 20, 24, 32, 40, 48 and stays at the cap', () => {
    const seen: number[] = [2]
    for (let i = 0; i < 11; i++) seen.push(nextCapacityStep(seen[seen.length - 1]!, 48))
    expect(seen).toEqual([2, 4, 6, 8, 10, 12, 16, 20, 24, 32, 40, 48])
    expect(nextCapacityStep(48, 48)).toBe(48)
  })

  it('stops at a custom maxLegs', () => {
    expect(nextCapacityStep(8, 10)).toBe(10)
    expect(nextCapacityStep(10, 10)).toBe(10)
    expect(nextCapacityStep(12, 10)).toBe(12)
  })
})

describe('capacityStepPasses', () => {
  it('fails on any failure', () => {
    expect(capacityStepPasses(evaluation({ failures: ['leg1 turn 1: x'] }), 2000, 1000)).toBe(false)
  })

  it('passes within baseline+delta', () => {
    expect(
      capacityStepPasses(evaluation({ finalLatencyMs: { p50: 1000, p95: 2900 } }), 2000, 1000),
    ).toBe(true)
    expect(
      capacityStepPasses(evaluation({ finalLatencyMs: { p50: 1000, p95: 3000 } }), 2000, 1000),
    ).toBe(true)
  })

  it('fails above baseline+delta', () => {
    expect(
      capacityStepPasses(evaluation({ finalLatencyMs: { p50: 1000, p95: 3100 } }), 2000, 1000),
    ).toBe(false)
  })

  it('fails when p95 is null', () => {
    expect(
      capacityStepPasses(evaluation({ finalLatencyMs: { p50: null, p95: null } }), 2000, 1000),
    ).toBe(false)
  })
})
