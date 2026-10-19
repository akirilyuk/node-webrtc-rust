import assert from 'node:assert/strict'
import test from 'node:test'

import { compareSummaries, renderComparison } from './compare.mjs'

const group = (vendor, metrics, extra = {}) => ({
  vendor,
  leg: 'tts',
  transport: 'http',
  model: 'm',
  phase: 'cold',
  text: 'short',
  n: 5,
  http_versions: [],
  metrics: Object.fromEntries(
    Object.entries(metrics).map(([k, median]) => [
      k,
      { n: 5, median, p90: median, min: median, max: median },
    ]),
  ),
  ...extra,
})

test('rows carry both medians, the delta and the percentage', () => {
  const a = { groups: [group('acme', { ttfa_ms: 100, total_ms: 400, only_a: 1 })] }
  const b = { groups: [group('acme', { ttfa_ms: 150, total_ms: 300 })] }
  const { rows } = compareSummaries(a, b)
  assert.equal(rows.length, 2)
  const ttfa = rows.find((r) => r.metric === 'ttfa_ms')
  assert.equal(ttfa.ours, 100)
  assert.equal(ttfa.yours, 150)
  assert.equal(ttfa.delta, 50)
  assert.equal(ttfa.delta_pct, 50)
  assert.equal(rows.find((r) => r.metric === 'total_ms').delta, -100)
})

test('groups on one side only are listed, not compared', () => {
  const a = { groups: [group('acme', { ttfa_ms: 1 }), group('onlyours', { ttfa_ms: 1 })] }
  const b = { groups: [group('acme', { ttfa_ms: 1 }), group('onlyyours', { ttfa_ms: 1 })] }
  const result = compareSummaries(a, b)
  assert.equal(result.rows.length, 1)
  assert.equal(result.onlyOurs.length, 1)
  assert.match(result.onlyOurs[0], /^onlyours /)
  assert.match(result.onlyYours[0], /^onlyyours /)
  const text = renderComparison(result)
  assert.match(text, /Only in ours/)
  assert.match(text, /Only in yours/)
})
