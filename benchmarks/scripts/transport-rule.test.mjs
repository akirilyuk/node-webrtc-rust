import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import { decideTransports, formatTable, readRecords } from './transport-rule.mjs'

const here = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(here, '../..')
const resultsDir = path.join(repoRoot, 'benchmarks/results/2026-10-19-ci-eu')

/** TTS records of one transport: `warm`/`cold` ttfa values, `stalls` by index (default 0). */
function tts(transport, { warm = [], cold = [], stalls = {}, vendor = 'v' }) {
  const rows = [...warm.map((v) => ['warm', v]), ...cold.map((v) => ['cold', v])]
  return rows.map(([phase, ttfa], i) => ({
    vendor,
    leg: 'tts',
    transport,
    model: 'm',
    phase,
    text: 'sentence',
    run: i,
    metrics: { ttfa_ms: ttfa, stall_ms: stalls[i] ?? 0 },
  }))
}

function stt(transport, { cold = [], warm = [], matched = {} }) {
  const rows = [...warm.map((v) => ['warm', v]), ...cold.map((v) => ['cold', v])]
  return rows.map(([phase, value], i) => ({
    vendor: 'v',
    leg: 'stt',
    transport,
    model: 'm',
    phase,
    text: 'phrase',
    run: i,
    metrics: { speech_end_to_final_ms: value, matched_words: matched[i] ?? 9 },
  }))
}

const repeat = (value, n) => Array.from({ length: n }, () => value)
const one = (records) => {
  const decisions = decideTransports(records)
  assert.equal(decisions.length, 1)
  return decisions[0]
}

test('the lower warm median wins', () => {
  const d = one([
    ...tts('http', { warm: repeat(400, 20), cold: repeat(500, 20) }),
    ...tts('websocket', { warm: repeat(200, 20), cold: repeat(900, 20) }),
  ])
  assert.equal(d.winner, 'websocket')
  assert.equal(d.reason, 'lower warm median')
})

test('a warm tie goes to the lower cold median', () => {
  // 100 vs 140 warm: within max(50, 10 %); cold 500 vs 200 is a clear difference.
  const d = one([
    ...tts('http', { warm: repeat(140, 20), cold: repeat(500, 20) }),
    ...tts('websocket', { warm: repeat(100, 20), cold: repeat(200, 20) }),
  ])
  assert.equal(d.winner, 'websocket')
  assert.equal(d.reason, 'warm tie, lower cold median')
})

test('a warm tie uses the 10 % band above 500 ms', () => {
  // 1000 vs 1090: within 10 % of 1000 (100 ms) but beyond the 50 ms floor.
  const d = one([
    ...tts('http', { warm: repeat(1090, 20), cold: repeat(1000, 20) }),
    ...tts('websocket', { warm: repeat(1000, 20), cold: repeat(1500, 20) }),
  ])
  assert.equal(d.winner, 'http')
  assert.equal(d.reason, 'warm tie, lower cold median')
})

test('a warm and cold tie goes to http', () => {
  const d = one([
    ...tts('websocket', { warm: repeat(100, 20), cold: repeat(300, 20) }),
    ...tts('http', { warm: repeat(130, 20), cold: repeat(330, 20) }),
  ])
  assert.equal(d.winner, 'http')
  assert.equal(d.reason, 'warm and cold tie, http')
})

test('one 20 ms stall in 40 samples qualifies', () => {
  const d = one([
    ...tts('http', { warm: repeat(100, 20), cold: repeat(100, 20), stalls: { 5: 20 } }),
    ...tts('websocket', { warm: repeat(400, 20), cold: repeat(400, 20) }),
  ])
  assert.equal(d.transports.find((t) => t.transport === 'http').qualifies, true)
  assert.equal(d.winner, 'http')
})

test('a 21 ms stall disqualifies', () => {
  const d = one([
    ...tts('http', { warm: repeat(100, 20), cold: repeat(100, 20), stalls: { 5: 21 } }),
    ...tts('websocket', { warm: repeat(400, 20), cold: repeat(400, 20) }),
  ])
  const http = d.transports.find((t) => t.transport === 'http')
  assert.equal(http.qualifies, false)
  assert.match(http.why, /21 ms/)
  assert.equal(d.winner, 'websocket')
  assert.equal(d.reason, 'single qualifying transport')
})

test('two stalls in 40 samples disqualify, and 39 samples allow none', () => {
  const two = one([
    ...tts('http', { warm: repeat(100, 20), cold: repeat(100, 20), stalls: { 3: 10, 7: 10 } }),
    ...tts('websocket', { warm: repeat(400, 20), cold: repeat(400, 20) }),
  ])
  assert.equal(two.transports.find((t) => t.transport === 'http').qualifies, false)
  assert.equal(two.winner, 'websocket')
  const short = one([
    ...tts('http', { warm: repeat(100, 19), cold: repeat(100, 20), stalls: { 3: 10 } }),
    ...tts('websocket', { warm: repeat(400, 20), cold: repeat(400, 20) }),
  ])
  assert.equal(short.transports.find((t) => t.transport === 'http').qualifies, false)
})

test('STT with a sample matching 6 of 9 words disqualifies', () => {
  const d = one([
    ...stt('http', { warm: repeat(900, 10), cold: repeat(900, 10), matched: { 4: 6 } }),
    ...stt('websocket', { warm: repeat(1500, 10), cold: repeat(1500, 10) }),
  ])
  const http = d.transports.find((t) => t.transport === 'http')
  assert.equal(http.qualifies, false)
  assert.match(http.why, /fewer than 7 of 9/)
  assert.equal(d.winner, 'websocket')
  const seven = one([
    ...stt('http', { warm: repeat(900, 10), cold: repeat(900, 10), matched: { 4: 7 } }),
    ...stt('websocket', { warm: repeat(1500, 10), cold: repeat(1500, 10) }),
  ])
  assert.equal(seven.winner, 'http')
})

test('a vendor and leg with one transport has no decision', () => {
  const d = one(tts('http', { warm: repeat(100, 20), cold: repeat(100, 20) }))
  assert.equal(d.winner, null)
  assert.equal(d.reason, 'single transport, no decision')
})

test('a transport without warm samples wins only as the single qualifying one', () => {
  const both = one([
    ...stt('http', { warm: repeat(1900, 10), cold: repeat(1900, 10) }),
    ...stt('websocket', { cold: repeat(1000, 10) }),
  ])
  assert.equal(both.winner, 'http')
  assert.equal(both.reason, 'only qualifying transport with warm samples')
  const alone = one([
    ...stt('http', { warm: repeat(1900, 10), cold: repeat(1900, 10), matched: { 0: 3 } }),
    ...stt('websocket', { cold: repeat(1000, 10) }),
  ])
  assert.equal(alone.winner, 'websocket')
  assert.equal(alone.reason, 'single qualifying transport')
})

test('no qualifying transport gives no winner', () => {
  const d = one([
    ...tts('http', { warm: repeat(100, 20), cold: repeat(100, 20), stalls: { 0: 50 } }),
    ...tts('websocket', { warm: repeat(100, 20), cold: repeat(100, 20), stalls: { 0: 50 } }),
  ])
  assert.equal(d.winner, null)
  assert.equal(d.reason, 'none qualifies')
})

test('the committed 2026-10-19 ci-eu results decide the published defaults', () => {
  const decisions = decideTransports(readRecords(path.join(resultsDir, 'raw.jsonl')))
  const get = (vendor, leg) => decisions.find((d) => d.vendor === vendor && d.leg === leg)
  assert.equal(get('elevenlabs', 'tts').winner, 'websocket')
  assert.equal(get('cartesia', 'tts').winner, 'http')
  assert.equal(get('fishaudio', 'tts').winner, 'http')
  const openai = get('openai', 'tts')
  assert.equal(openai.winner, null)
  const http = openai.transports.find((t) => t.transport === 'http')
  assert.equal(http.qualifies, false)
  assert.match(http.why, /26 of 40 samples stalled/)
})

test('the CLI prints a table per vendor and leg', () => {
  const result = spawnSync(process.execPath, [path.join(here, 'transport-rule.mjs'), resultsDir], {
    encoding: 'utf8',
  })
  assert.equal(result.status, 0, result.stderr)
  assert.match(result.stdout, /### elevenlabs tts/)
  assert.match(result.stdout, /Winner: websocket \(lower warm median\)/)
  assert.ok(formatTable(decideTransports([])) === '')
})
