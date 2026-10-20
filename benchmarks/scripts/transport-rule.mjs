// The default transport rule (METHODOLOGY.md, "Default transport rule") as code. Reads bench
// records (raw.jsonl lines) and decides, per vendor and leg, which transport should be the default.
//
//   npm run bench:transport-table -- benchmarks/results/<folder>

import { readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

/** One TTS audio frame; a stall above this is audible. */
export const MAX_STALL_MS = 20
/** At most one stalled sample in this many (`floor(n / 40)`, 2.5 %). */
export const STALL_SAMPLES_PER = 40
/** STT: of the 9 words of the test phrase, at least this many must match in every sample. */
export const MIN_MATCHED_WORDS = 7
/** Two medians tie when they differ by at most max(TIE_FLOOR_MS, TIE_RATIO x the lower). */
export const TIE_FLOOR_MS = 50
export const TIE_RATIO = 0.1

const METRIC = { tts: 'ttfa_ms', stt: 'speech_end_to_final_ms' }

export function median(values) {
  if (values.length === 0) return null
  const sorted = [...values].sort((a, b) => a - b)
  const mid = sorted.length >> 1
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2
}

const tied = (a, b) => Math.abs(a - b) <= Math.max(TIE_FLOOR_MS, TIE_RATIO * Math.min(a, b))

/** `{ qualifies, why }` for the records of one transport. `why` is empty when it qualifies. */
function qualify(leg, records) {
  const n = records.length
  if (leg === 'tts') {
    const stalls = records.map((r) => r.metrics.stall_ms ?? 0)
    const big = Math.max(...stalls)
    const stalled = stalls.filter((s) => s > 0).length
    const allowed = Math.floor(n / STALL_SAMPLES_PER)
    if (big > MAX_STALL_MS) {
      return {
        qualifies: false,
        why: `${stalled} of ${n} samples stalled, largest ${big} ms (limit ${MAX_STALL_MS} ms)`,
      }
    }
    if (stalled > allowed) {
      return { qualifies: false, why: `${stalled} of ${n} samples stalled (limit ${allowed})` }
    }
    return { qualifies: true, why: '' }
  }
  const low = records.filter((r) => !(r.metrics.matched_words >= MIN_MATCHED_WORDS)).length
  if (low > 0) {
    return {
      qualifies: false,
      why: `${low} of ${n} samples matched fewer than ${MIN_MATCHED_WORDS} of 9 words`,
    }
  }
  return { qualifies: true, why: '' }
}

/**
 * Decides the default transport of every vendor and leg in `records`.
 * Returns `[{ vendor, leg, metric, transports, winner, reason }]`; `transports` has, per
 * transport, `{ transport, model, n, warm, cold, qualifies, why }` (`warm`/`cold`: median of the
 * leg's metric, `null` without samples). `winner` is a transport name or `null`.
 */
export function decideTransports(records) {
  const groups = new Map()
  for (const r of records) {
    const key = `${r.vendor}|${r.leg}`
    if (!groups.has(key)) groups.set(key, new Map())
    const byTransport = groups.get(key)
    if (!byTransport.has(r.transport)) byTransport.set(r.transport, [])
    byTransport.get(r.transport).push(r)
  }
  const decisions = []
  for (const [key, byTransport] of groups) {
    const [vendor, leg] = key.split('|')
    const metric = METRIC[leg]
    if (!metric) continue
    const transports = [...byTransport].map(([transport, recs]) => {
      const values = (phase) =>
        recs
          .filter((r) => r.phase === phase && Number.isFinite(r.metrics[metric]))
          .map((r) => r.metrics[metric])
      return {
        transport,
        model: recs[0].model,
        n: recs.length,
        warm: median(values('warm')),
        cold: median(values('cold')),
        ...qualify(leg, recs),
      }
    })
    const { winner, reason } = decide(transports)
    decisions.push({ vendor, leg, metric, transports, winner, reason })
  }
  return decisions
}

function decide(transports) {
  if (transports.length === 1) return { winner: null, reason: 'single transport, no decision' }
  const qualified = transports.filter((t) => t.qualifies)
  if (qualified.length === 0) return { winner: null, reason: 'none qualifies' }
  if (qualified.length === 1) {
    return { winner: qualified[0].transport, reason: 'single qualifying transport' }
  }
  // Only a transport with a warm median can win against another qualifying one.
  const ranked = qualified.filter((t) => t.warm !== null).sort((a, b) => a.warm - b.warm)
  if (ranked.length === 0)
    return { winner: null, reason: 'no qualifying transport has warm samples' }
  if (ranked.length === 1) {
    return { winner: ranked[0].transport, reason: 'only qualifying transport with warm samples' }
  }
  const [a, b] = ranked
  if (!tied(a.warm, b.warm)) return { winner: a.transport, reason: 'lower warm median' }
  if (a.cold !== null && b.cold !== null && !tied(a.cold, b.cold)) {
    const lower = a.cold < b.cold ? a : b
    return { winner: lower.transport, reason: 'warm tie, lower cold median' }
  }
  const http = [a, b].find((t) => t.transport === 'http')
  return {
    winner: (http ?? a).transport,
    reason: http ? 'warm and cold tie, http' : 'warm and cold tie, lower warm median',
  }
}

const fmt = (v) => (v === null ? '-' : String(Math.round(v * 10) / 10))

/** Markdown: one table per vendor and leg. */
export function formatTable(decisions) {
  const out = []
  for (const d of decisions) {
    out.push(`### ${d.vendor} ${d.leg} (${d.metric})`, '')
    out.push('| transport | model | n | median warm | median cold | qualifies |')
    out.push('| --- | --- | --- | --- | --- | --- |')
    for (const t of d.transports) {
      out.push(
        `| ${t.transport} | ${t.model} | ${t.n} | ${fmt(t.warm)} | ${fmt(t.cold)} | ${t.qualifies ? 'yes' : `no: ${t.why}`} |`,
      )
    }
    out.push('', `Winner: ${d.winner ?? 'none'} (${d.reason})`, '')
  }
  return out.join('\n')
}

export function readRecords(file) {
  return readFileSync(file, 'utf8')
    .split('\n')
    .filter((l) => l.trim() !== '')
    .map((l) => JSON.parse(l))
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const dir = process.argv[2]
  if (!dir) {
    process.stderr.write('usage: transport-rule.mjs <results-dir>\n')
    process.exit(2)
  }
  process.stdout.write(
    `${formatTable(decideTransports(readRecords(path.join(dir, 'raw.jsonl'))))}\n`,
  )
}
