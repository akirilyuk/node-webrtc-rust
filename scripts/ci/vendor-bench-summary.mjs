#!/usr/bin/env node
// Summarises `vendor_bench {json}` lines (crates/vendor-live-support/src/bench.rs) from a bench log.
//
//   node scripts/ci/vendor-bench-summary.mjs [log-path|-] [out.json]
//
// Reads the log from the path in argument 1, or from stdin when it is missing or "-". Prints a
// markdown table per leg (median / p90 of every numeric metric, sample count n, and the
// http_version values seen) on stdout, and writes the grouped JSON to the path in argument 2.
// Groups are vendor / leg / transport / model / phase / text. Only `vendor_bench` lines are read,
// so nothing else in the log (and no transcript) ends up in the output.

import { readFileSync, writeFileSync } from 'node:fs'
import { pathToFileURL } from 'node:url'

const MARKER = 'vendor_bench '

/** Records from every line that contains `vendor_bench {json}` (libtest may prefix the line). */
export function parseBenchLines(text) {
  const records = []
  for (const line of text.split('\n')) {
    const at = line.indexOf(MARKER)
    if (at < 0) continue
    const json = line.slice(at + MARKER.length).trim()
    if (!json.startsWith('{')) continue
    try {
      const record = JSON.parse(json)
      if (record && typeof record === 'object' && record.metrics) records.push(record)
    } catch {
      process.stderr.write('vendor-bench-summary: skipped an unparsable vendor_bench line\n')
    }
  }
  return records
}

/** Median of a sorted-ascending array (mean of the middle two for an even count). */
export function median(sorted) {
  const mid = sorted.length >> 1
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2
}

/** Nearest-rank percentile of a sorted-ascending array. */
export function percentile(sorted, p) {
  return sorted[Math.max(0, Math.ceil((p / 100) * sorted.length) - 1)]
}

/**
 * Adds derived metrics to a record (a copy): for a TTS sample with `total_ms` and `audio_ms > 0`,
 * `real_time_factor = total_ms / audio_ms` (below 1 means faster than real time).
 */
export function withDerivedMetrics(record) {
  const m = record.metrics
  if (
    record.leg === 'tts' &&
    m &&
    typeof m.total_ms === 'number' &&
    typeof m.audio_ms === 'number' &&
    m.audio_ms > 0
  ) {
    return { ...record, metrics: { ...m, real_time_factor: m.total_ms / m.audio_ms } }
  }
  return record
}

const KEY_FIELDS = ['vendor', 'leg', 'transport', 'model', 'phase', 'text']

/** Groups records and computes median / p90 / min / max per numeric metric. */
export function summarize(records) {
  const groups = new Map()
  for (const record of records) {
    const key = KEY_FIELDS.map((f) => String(record[f])).join('\u0000')
    if (!groups.has(key)) {
      groups.set(key, {
        ...Object.fromEntries(KEY_FIELDS.map((f) => [f, record[f]])),
        n: 0,
        http_versions: new Set(),
        values: new Map(),
      })
    }
    const group = groups.get(key)
    group.n += 1
    if (record.http_version) group.http_versions.add(record.http_version)
    for (const [name, value] of Object.entries(record.metrics)) {
      if (typeof value !== 'number' || !Number.isFinite(value)) continue
      if (!group.values.has(name)) group.values.set(name, [])
      group.values.get(name).push(value)
    }
  }
  return [...groups.values()]
    .map((g) => ({
      ...Object.fromEntries(KEY_FIELDS.map((f) => [f, g[f]])),
      n: g.n,
      http_versions: [...g.http_versions].sort(),
      metrics: Object.fromEntries(
        [...g.values.entries()].map(([name, values]) => {
          const sorted = [...values].sort((a, b) => a - b)
          return [
            name,
            {
              n: sorted.length,
              median: median(sorted),
              p90: percentile(sorted, 90),
              min: sorted[0],
              max: sorted[sorted.length - 1],
            },
          ]
        }),
      ),
    }))
    .sort(
      (a, b) =>
        KEY_FIELDS.map((f) => String(a[f]).localeCompare(String(b[f]))).find((c) => c !== 0) || 0,
    )
}

function fmt(value) {
  return Number.isInteger(value) ? String(value) : value.toFixed(1)
}

/** One markdown table per leg: group columns, n, http, then `median / p90` per metric. */
export function renderMarkdown(groups) {
  if (groups.length === 0) return '_No `vendor_bench` lines in the log._\n'
  const out = []
  for (const leg of [...new Set(groups.map((g) => g.leg))].sort()) {
    const rows = groups.filter((g) => g.leg === leg)
    const metrics = [...new Set(rows.flatMap((g) => Object.keys(g.metrics)))]
    out.push(`### ${leg.toUpperCase()} (median / p90)\n`)
    out.push(`| vendor | transport | model | phase | text | n | http | ${metrics.join(' | ')} |`)
    out.push(`|${' --- |'.repeat(7 + metrics.length)}`)
    for (const g of rows) {
      const cells = metrics.map((m) =>
        g.metrics[m] ? `${fmt(g.metrics[m].median)} / ${fmt(g.metrics[m].p90)}` : '',
      )
      out.push(
        `| ${[g.vendor, g.transport, g.model, g.phase, g.text, g.n, g.http_versions.join(', '), ...cells].join(' | ')} |`,
      )
    }
    out.push('')
  }
  return out.join('\n')
}

function main(argv) {
  const [input, outPath] = argv
  const text = input && input !== '-' ? readFileSync(input, 'utf8') : readFileSync(0, 'utf8')
  const groups = summarize(parseBenchLines(text).map(withDerivedMetrics))
  if (outPath) writeFileSync(outPath, `${JSON.stringify(groups, null, 2)}\n`)
  process.stdout.write(renderMarkdown(groups))
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2))
}
