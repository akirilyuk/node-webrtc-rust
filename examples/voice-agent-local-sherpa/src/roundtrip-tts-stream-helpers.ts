/**
 * Pure evaluators for TTS stream-chunks roundtrip (no Sherpa models required).
 */

export interface TtsStreamLatencySample {
  mode: 'streaming' | 'buffered'
  /** sendTextToTTS → speaker agent_speaking_start (ms) */
  firstAudioMs: number
  recognized: string
}

export interface TtsStreamCompareResult {
  ok: boolean
  failures: string[]
  streamingMs: number
  bufferedMs: number
  minImprovementMs: number
  maxRegressionMs: number
}

/**
 * Compare first-audio latency (streaming vs buffered).
 *
 * Piper/VITS progress callbacks can land near the end of a short/fast synth, so
 * the default is "streaming must not be much worse" (`maxRegressionMs`). Set
 * `minImprovementMs` (env `SHERPA_TTS_STREAM_MIN_IMPROVEMENT_MS`) when a clear
 * win is required on a slower model / longer phrase.
 */
export function evaluateStreamingFasterOrEqual(params: {
  streamingMs: number
  bufferedMs: number
  /** Require streaming to beat buffered by at least this many ms (default 0). */
  minImprovementMs?: number
  /** Allow streaming this many ms slower than buffered (timer / scheduler jitter). */
  maxRegressionMs?: number
}): TtsStreamCompareResult {
  const minImprovementMs = params.minImprovementMs ?? 0
  const maxRegressionMs = params.maxRegressionMs ?? 80
  const failures: string[] = []
  const limit = params.bufferedMs - minImprovementMs + maxRegressionMs
  if (!(params.streamingMs <= limit)) {
    failures.push(
      `streaming first-audio (${params.streamingMs}ms) exceeds limit ${limit.toFixed(0)}ms ` +
        `(buffered=${params.bufferedMs}ms, minImprovement=${minImprovementMs}ms, maxRegression=${maxRegressionMs}ms)`,
    )
  }
  return {
    ok: failures.length === 0,
    failures,
    streamingMs: params.streamingMs,
    bufferedMs: params.bufferedMs,
    minImprovementMs,
    maxRegressionMs,
  }
}

export function evaluateRecognizedContainsWords(params: {
  recognized: string
  requiredWords: string[]
  label: string
}): { ok: boolean; failures: string[] } {
  const normalized = params.recognized.toLowerCase().replace(/[^a-z0-9\s]/g, ' ')
  const failures: string[] = []
  for (const word of params.requiredWords) {
    if (!normalized.includes(word.toLowerCase())) {
      failures.push(`${params.label}: missing word "${word}" in "${params.recognized}"`)
    }
  }
  return { ok: failures.length === 0, failures }
}
