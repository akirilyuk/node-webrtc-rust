// Run: node --test scripts/perf/perf-charts.test.mjs
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

import { renderChart } from './perf-charts.mjs'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..')
const SPEC = join(ROOT, 'docs/perf-data/2026-10-16/charts.json')
const OUT = join(ROOT, 'docs/perf-data/charts')

test('committed chart SVGs match charts.json', () => {
  const spec = JSON.parse(readFileSync(SPEC, 'utf8'))
  for (const chart of spec.charts) {
    const committed = readFileSync(join(OUT, `${chart.id}.svg`), 'utf8')
    assert.equal(
      committed,
      renderChart(chart),
      `${chart.id}.svg is stale: re-run scripts/perf/perf-charts.mjs`,
    )
  }
})

test('hbar labels every bar and carries light and dark colors', () => {
  const svg = renderChart({
    kind: 'hbar',
    title: 'T',
    rows: ['a', 'b'],
    series: [
      { name: 'old', slot: 0, values: [10, 20], labels: ['10 ms', '20 ms'] },
      { name: 'new', slot: 2, values: [0, 5] },
    ],
  })
  for (const text of ['10 ms', '20 ms', '>0<', '>5<', '>old<', '>new<'])
    assert.ok(svg.includes(text), text)
  assert.ok(svg.includes('@media (prefers-color-scheme: dark)'))
  assert.ok(svg.includes('#2a78d6') && svg.includes('#3987e5'), 'slot 0 light and dark')
})

test('log-scale hbar spans the decades of its values', () => {
  const svg = renderChart({
    kind: 'hbar',
    scale: 'log',
    title: 'T',
    rows: ['a'],
    series: [{ name: 's', slot: 0, values: [3, 57000] }],
  })
  for (const tick of ['>1<', '>10<', '>100000<']) assert.ok(svg.includes(tick), tick)
})

test('line end label next to the threshold moves off the rule', () => {
  const svg = renderChart({
    kind: 'line',
    title: 'T',
    yMin: 0,
    yMax: 100,
    xMax: 10,
    threshold: { value: 50, label: 'budget' },
    series: [{ name: 'near', slot: 0, points: [[10, 51]] }],
  })
  const rule = Number(svg.match(/class="thr" x1="[\d.]+" y1="([\d.]+)"/)[1])
  const label = Number(
    svg.match(/y="([\d.]+)" font-size="13" font-weight="600" text-anchor="start">near</)[1],
  )
  assert.ok(Math.abs(label - 4 - rule) >= 10, `label at ${label}, rule at ${rule}`)
})
