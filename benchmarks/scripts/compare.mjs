#!/usr/bin/env node
// Compares two bench result folders (or summary.json files) and prints markdown.
//
//   node benchmarks/scripts/compare.mjs <ours> <yours>

import { existsSync, readFileSync, statSync } from 'node:fs'
import path from 'node:path'
import { pathToFileURL } from 'node:url'

const KEY_FIELDS = ['vendor', 'leg', 'transport', 'model', 'phase', 'text']

const groupKey = (g) => KEY_FIELDS.map((f) => String(g[f])).join(' / ')

/**
 * Rows for every metric present in both summaries, plus the groups only one side has.
 * @param {{ groups: any[] }} a
 * @param {{ groups: any[] }} b
 */
export function compareSummaries(a, b) {
  const left = new Map(a.groups.map((g) => [groupKey(g), g]))
  const right = new Map(b.groups.map((g) => [groupKey(g), g]))
  const rows = []
  for (const [key, ga] of left) {
    const gb = right.get(key)
    if (!gb) continue
    for (const [metric, ma] of Object.entries(ga.metrics)) {
      const mb = gb.metrics[metric]
      if (!mb) continue
      const delta = mb.median - ma.median
      rows.push({
        group: key,
        metric,
        ours: ma.median,
        yours: mb.median,
        delta,
        delta_pct: ma.median === 0 ? null : (delta / ma.median) * 100,
      })
    }
  }
  return {
    rows,
    onlyOurs: [...left.keys()].filter((k) => !right.has(k)),
    onlyYours: [...right.keys()].filter((k) => !left.has(k)),
  }
}

const fmt = (n) => (n === null ? '' : Number.isInteger(n) ? String(n) : n.toFixed(1))

export function renderComparison({ rows, onlyOurs, onlyYours }) {
  const out = [
    '| group | metric | ours (median) | yours (median) | delta | delta % |',
    '| --- | --- | --- | --- | --- | --- |',
  ]
  for (const r of rows) {
    out.push(
      `| ${r.group} | ${r.metric} | ${fmt(r.ours)} | ${fmt(r.yours)} | ${fmt(r.delta)} | ${fmt(r.delta_pct)} |`,
    )
  }
  out.push('')
  if (onlyOurs.length > 0) out.push('**Only in ours:**', ...onlyOurs.map((k) => `- ${k}`), '')
  if (onlyYours.length > 0) out.push('**Only in yours:**', ...onlyYours.map((k) => `- ${k}`), '')
  return out.join('\n')
}

function load(target) {
  const summaryPath = statSync(target).isDirectory() ? path.join(target, 'summary.json') : target
  const summary = JSON.parse(readFileSync(summaryPath, 'utf8'))
  const envPath = path.join(path.dirname(summaryPath), 'environment.json')
  const environment = existsSync(envPath) ? JSON.parse(readFileSync(envPath, 'utf8')) : null
  return { summary, environment }
}

function describe(label, { summary, environment }) {
  const e = environment
  const parts = [`region ${summary.region ?? e?.region ?? 'unknown'}`]
  if (e) parts.push(`nwr ${e.nwr_version}`, `${e.cpu_model} x${e.cpu_cores}`)
  return `- ${label}: ${parts.join(', ')}`
}

function main(argv) {
  if (argv.length !== 2) {
    process.stderr.write('usage: compare.mjs <ours> <yours>\n')
    return 2
  }
  let ours
  let yours
  try {
    ours = load(argv[0])
    yours = load(argv[1])
  } catch (err) {
    process.stderr.write(`compare: ${err.message}\n`)
    return 2
  }
  process.stdout.write([describe('ours', ours), describe('yours', yours), ''].join('\n') + '\n')
  process.stdout.write(renderComparison(compareSummaries(ours.summary, yours.summary)))
  return 0
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exit(main(process.argv.slice(2)))
}
