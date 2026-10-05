/**
 * Pure helpers for the Sherpa load roundtrip (`roundtrip-load.ts`): percentiles, per-turn
 * evaluation and the capacity-mode step sequence. No native imports.
 */

import { finalContainsKeyword } from './roundtrip-concurrent-timing-helpers.js'

/** Nearest-rank percentile (`p` in 0–100). `null` for an empty input. */
export function percentile(values: number[], p: number): number | null {
  if (values.length === 0) return null
  const sorted = [...values].sort((a, b) => a - b)
  const rank = Math.ceil((p / 100) * sorted.length)
  const index = Math.min(sorted.length - 1, Math.max(0, rank - 1))
  return sorted[index]!
}

export type LoadTurnResult = {
  legId: string
  turn: number
  keyword: string
  finalText: string | null
  finalLatencyMs: number | null
  ttsStartLatencyMs: number | null
  error?: string
}

export type LatencyStats = { p50: number | null; p95: number | null }

export type LoadRunEvaluation = {
  failures: string[]
  finalLatencyMs: LatencyStats
  ttsStartLatencyMs: LatencyStats
  turnSuccessRate: number
}

function latencyStats(values: Array<number | null>): LatencyStats {
  const present = values.filter((v): v is number => v != null)
  return { p50: percentile(present, 50), p95: percentile(present, 95) }
}

/** A turn fails on `error`, a missing final, or a final without the leg keyword. */
export function evaluateLoadRun(turns: LoadTurnResult[]): LoadRunEvaluation {
  const failures: string[] = []
  for (const turn of turns) {
    const prefix = `${turn.legId} turn ${turn.turn}`
    if (turn.error) {
      failures.push(`${prefix}: error: ${turn.error}`)
    } else if (turn.finalText == null) {
      failures.push(`${prefix}: missing user_speech_final`)
    } else if (!finalContainsKeyword(turn.finalText, turn.keyword)) {
      failures.push(
        `${prefix}: final "${turn.finalText.slice(0, 80)}" missing keyword "${turn.keyword}"`,
      )
    }
  }
  return {
    failures,
    finalLatencyMs: latencyStats(turns.map((t) => t.finalLatencyMs)),
    ttsStartLatencyMs: latencyStats(turns.map((t) => t.ttsStartLatencyMs)),
    turnSuccessRate: turns.length === 0 ? 0 : (turns.length - failures.length) / turns.length,
  }
}

export const CAPACITY_STEPS = [2, 4, 6, 8, 10, 12, 16, 20, 24, 32, 40, 48] as const

/** Next leg count in the capacity sequence, never above `maxLegs`; `current` once it is reached. */
export function nextCapacityStep(current: number, maxLegs: number): number {
  if (current >= maxLegs) return current
  const next = CAPACITY_STEPS.find((step) => step > current)
  if (next === undefined) return current
  return Math.min(next, maxLegs)
}

/**
 * A capacity step passes with zero failures and a final-latency p95 no more than `maxDeltaMs`
 * above the 2-leg baseline p95 (final latency has a fixed VAD/gate-hold floor, so an absolute
 * SLO cannot detect saturation).
 */
export function capacityStepPasses(
  evaluation: LoadRunEvaluation,
  baselineP95Ms: number,
  maxDeltaMs: number,
): boolean {
  return (
    evaluation.failures.length === 0 &&
    evaluation.finalLatencyMs.p95 !== null &&
    evaluation.finalLatencyMs.p95 <= baselineP95Ms + maxDeltaMs
  )
}
