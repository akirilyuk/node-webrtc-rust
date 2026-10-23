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

/**
 * Default `maxRatio` for {@link evaluateRelativeSpread}: the spread between legs may be at most
 * the fastest leg's own latency. n legs run one after another spread by (n - 1) latencies, which
 * is about 2x the fastest one for 3 legs, so serialized work fails; concurrent legs on a slow or
 * loaded host stay together because every leg slows down with the host.
 */
export const DEFAULT_RELATIVE_SPREAD_RATIO = 1

/**
 * Concurrency oracle that does not depend on host speed. Latency of a leg is its timestamp minus
 * `startedAtMs` (the common request time). Passes when `max - min <= maxRatio * min(latency)`.
 */
export function evaluateRelativeSpread(
  timestampsMs: number[],
  startedAtMs: number,
  maxRatio: number = DEFAULT_RELATIVE_SPREAD_RATIO,
): ConcurrentWindowEvaluation {
  if (timestampsMs.length === 0) {
    return { passed: false, spreadMs: 0, failures: ['no timestamps recorded'] }
  }
  const latencies = timestampsMs.map((t) => t - startedAtMs)
  const minLatency = Math.min(...latencies)
  const spreadMs = Math.max(...latencies) - minLatency
  const budget = maxRatio * Math.max(minLatency, 0)
  const failures: string[] = []
  if (spreadMs > budget) {
    failures.push(
      `legs finished ${spreadMs.toFixed(0)}ms apart, more than ${maxRatio}x the fastest leg's own latency (${minLatency.toFixed(0)}ms): work looks serialized instead of concurrent`,
    )
  }
  return { passed: failures.length === 0, spreadMs, failures }
}

/** Distinct phrases — keyword is a token Sherpa usually preserves (not always NATO). */
// Qualified in `stt-phrase-qualification.json`; `alpha` was misheard as "Halfa"/"half a" in 4 of 300 clean syntheses.
export const LEG_CATALOG = [
  { phrase: 'charlie one two three', keyword: 'charlie' },
  { phrase: 'bravo four five six', keyword: 'four' },
  { phrase: 'delta seven eight nine', keyword: 'delta' },
  { phrase: 'echo ten eleven twelve', keyword: 'ten' },
  { phrase: 'foxtrot thirteen fourteen fifteen', keyword: 'thirteen' },
  { phrase: 'golf sixteen seventeen eighteen', keyword: 'sixteen' },
] as const

/**
 * Known Sherpa STT confusions for concurrent-roundtrip keywords.
 * Keys and values are matched after {@link normalizeForKeyword}.
 */
const KEYWORD_ALIASES: Record<string, readonly string[]> = {
  // "delta" often lands as "belta" / "belt a" under concurrent TTS crosstalk.
  delta: ['belta', 'belt a'],
  // "four" and "for" are homophones; STT cannot tell them apart (test.wrong-oracle).
  four: ['for'],
}

export function finalContainsKeyword(finalText: string, keyword: string): boolean {
  const haystack = normalizeForKeyword(finalText)
  const needle = normalizeForKeyword(keyword)
  if (haystack.includes(needle)) return true
  const aliases = KEYWORD_ALIASES[needle] ?? []
  return aliases.some((alias) => haystack.includes(normalizeForKeyword(alias)))
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
  /** Absolute override (ms). Omit to use the relative oracle with `startedAtMs`. */
  maxAgentStartSpreadMs?: number
  /** Absolute override (ms). Omit to use the relative oracle with `startedAtMs`. */
  maxFinalSpreadMs?: number
  /** Common request time (same clock as the leg timestamps), for the relative oracle. */
  startedAtMs?: number
  /** Ratio for the relative oracle; defaults to {@link DEFAULT_RELATIVE_SPREAD_RATIO}. */
  maxSpreadRatio?: number
}): ConcurrentRoundtripEvaluation {
  const spreadWindow = (
    timestamps: number[],
    absoluteMaxMs: number | undefined,
  ): ConcurrentWindowEvaluation => {
    if (absoluteMaxMs != null) return evaluateConcurrentWindow(timestamps, absoluteMaxMs)
    if (params.startedAtMs == null) {
      throw new Error('evaluateConcurrentRoundtrip needs startedAtMs or an absolute spread limit')
    }
    return evaluateRelativeSpread(timestamps, params.startedAtMs, params.maxSpreadRatio)
  }
  const failures: string[] = []
  if (params.enqueueMs > params.maxEnqueueMs) {
    failures.push(
      `enqueue took ${params.enqueueMs.toFixed(0)}ms (max ${params.maxEnqueueMs}ms) — nonBlocking may not be active`,
    )
  }

  const agentStarts = params.legs
    .map((leg) => leg.agentSpeakingStartMs)
    .filter((t): t is number => t != null)
  const agentStartWindow = spreadWindow(agentStarts, params.maxAgentStartSpreadMs)
  failures.push(...agentStartWindow.failures)

  const finals = params.legs.map((leg) => leg.finalMs).filter((t): t is number => t != null)
  const finalWindow = spreadWindow(finals, params.maxFinalSpreadMs)
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
