/**
 * Pure helpers for concurrent multi-leg Sherpa roundtrip timing assertions.
 */

export interface ConcurrentWindowEvaluation {
  passed: boolean
  spreadMs: number
  failures: string[]
}

/** Pass when every timestamp falls within `maxSpreadMs` of the earliest. */
export function evaluateConcurrentWindow(
  timestampsMs: number[],
  maxSpreadMs: number,
): ConcurrentWindowEvaluation {
  const failures: string[] = []
  if (timestampsMs.length === 0) {
    return { passed: false, spreadMs: 0, failures: ['no timestamps recorded'] }
  }
  const min = Math.min(...timestampsMs)
  const max = Math.max(...timestampsMs)
  const spreadMs = max - min
  if (spreadMs > maxSpreadMs) {
    failures.push(
      `timestamps spread ${spreadMs.toFixed(0)}ms exceeds ${maxSpreadMs}ms window (min=${min.toFixed(0)} max=${max.toFixed(0)})`,
    )
  }
  return { passed: failures.length === 0, spreadMs, failures }
}

export function finalContainsKeyword(finalText: string, keyword: string): boolean {
  return normalizeForKeyword(finalText).includes(normalizeForKeyword(keyword))
}

function normalizeForKeyword(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9\s]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}

export interface ConcurrentLegResult {
  legId: string
  phrase: string
  keyword: string
  agentSpeakingStartMs: number | null
  finalText: string | null
  finalMs: number | null
}

export interface ConcurrentRoundtripEvaluation {
  passed: boolean
  failures: string[]
  enqueueMs: number
  agentStartWindow: ConcurrentWindowEvaluation
  finalWindow: ConcurrentWindowEvaluation
}

export function evaluateConcurrentRoundtrip(params: {
  legs: ConcurrentLegResult[]
  enqueueMs: number
  maxEnqueueMs: number
  maxAgentStartSpreadMs: number
  maxFinalSpreadMs: number
}): ConcurrentRoundtripEvaluation {
  const failures: string[] = []
  if (params.enqueueMs > params.maxEnqueueMs) {
    failures.push(
      `enqueue took ${params.enqueueMs.toFixed(0)}ms (max ${params.maxEnqueueMs}ms) — nonBlocking may not be active`,
    )
  }

  const agentStarts = params.legs
    .map((leg) => leg.agentSpeakingStartMs)
    .filter((t): t is number => t != null)
  const agentStartWindow = evaluateConcurrentWindow(agentStarts, params.maxAgentStartSpreadMs)
  failures.push(...agentStartWindow.failures)

  const finals = params.legs.map((leg) => leg.finalMs).filter((t): t is number => t != null)
  const finalWindow = evaluateConcurrentWindow(finals, params.maxFinalSpreadMs)
  failures.push(...finalWindow.failures)

  for (const leg of params.legs) {
    if (!leg.finalText?.trim()) {
      failures.push(`${leg.legId}: missing user_speech_final`)
      continue
    }
    if (!finalContainsKeyword(leg.finalText, leg.keyword)) {
      failures.push(
        `${leg.legId}: final "${leg.finalText.slice(0, 80)}" missing keyword "${leg.keyword}"`,
      )
    }
  }

  return {
    passed: failures.length === 0,
    failures,
    enqueueMs: params.enqueueMs,
    agentStartWindow,
    finalWindow,
  }
}
