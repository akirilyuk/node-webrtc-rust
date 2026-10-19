import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

import {
  median,
  parseBenchLines,
  percentile,
  renderMarkdown,
  summarize,
} from './vendor-bench-summary.mjs'

const script = join(dirname(fileURLToPath(import.meta.url)), 'vendor-bench-summary.mjs')

function line(run, ttfa, extra = {}) {
  return `vendor_bench ${JSON.stringify({
    vendor: 'acme',
    leg: 'tts',
    transport: 'http',
    model: 'm1',
    phase: 'warm',
    text: 'short',
    run,
    http_version: 'h2',
    metrics: { ttfa_ms: ttfa, chunks: 4, first_before_return: true, ...extra },
  })}`
}

// libtest with --nocapture prefixes the first output line of a test with "test name ... ".
const LOG = [
  'running 2 tests',
  `test acme_tts ... ${line(0, 100)}`,
  line(1, 200),
  line(2, 300),
  line(3, 400),
  'some other output with a transcript: the quick brown fox',
  'vendor_bench {not json',
  `vendor_bench ${JSON.stringify({
    vendor: 'acme',
    leg: 'stt',
    transport: 'websocket',
    model: 's1',
    phase: 'cold',
    text: 'phrase',
    run: 0,
    http_version: null,
    metrics: { open_ms: 50, first_partial_ms: null, speech_end_to_final_ms: 700 },
  })}`,
  'test result: ok. 2 passed',
].join('\n')

test('parses vendor_bench lines, also behind a libtest prefix, and skips junk', () => {
  const records = parseBenchLines(LOG)
  assert.equal(records.length, 5)
  assert.equal(records[0].metrics.ttfa_ms, 100)
})

test('median and p90', () => {
  assert.equal(median([1, 2, 3]), 2)
  assert.equal(median([1, 2, 3, 4]), 2.5)
  assert.equal(percentile([100, 200, 300, 400], 90), 400)
  assert.equal(
    percentile(
      Array.from({ length: 10 }, (_, i) => i + 1),
      90,
    ),
    9,
  )
  assert.equal(percentile([5], 90), 5)
})

test('groups by vendor/leg/transport/model/phase/text with n and http versions', () => {
  const groups = summarize(parseBenchLines(LOG))
  assert.equal(groups.length, 2)
  const tts = groups.find((g) => g.leg === 'tts')
  assert.equal(tts.n, 4)
  assert.deepEqual(tts.http_versions, ['h2'])
  assert.equal(tts.metrics.ttfa_ms.median, 250)
  assert.equal(tts.metrics.ttfa_ms.p90, 400)
  assert.equal(tts.metrics.chunks.median, 4)
  assert.equal(tts.metrics.first_before_return, undefined, 'booleans are not metrics')
  const stt = groups.find((g) => g.leg === 'stt')
  assert.equal(stt.metrics.first_partial_ms, undefined, 'null metrics are skipped')
  assert.equal(stt.metrics.speech_end_to_final_ms.median, 700)
})

test('markdown has a table per leg and never repeats log text outside vendor_bench lines', () => {
  const markdown = renderMarkdown(summarize(parseBenchLines(LOG)))
  assert.match(markdown, /### STT/)
  assert.match(markdown, /### TTS/)
  assert.match(markdown, /\| acme \| http \| m1 \| warm \| short \| 4 \| h2 \|/)
  assert.match(markdown, /250 \/ 400/)
  assert.doesNotMatch(markdown, /quick brown fox/)
  assert.match(renderMarkdown([]), /No `vendor_bench` lines/)
})

test('cli reads a path, prints markdown and writes grouped json', () => {
  const dir = mkdtempSync(join(tmpdir(), 'vendor-bench-'))
  const log = join(dir, 'bench.log')
  const json = join(dir, 'bench.json')
  writeFileSync(log, LOG)
  const stdout = execFileSync('node', [script, log, json], { encoding: 'utf8' })
  assert.match(stdout, /### TTS/)
  const groups = JSON.parse(readFileSync(json, 'utf8'))
  assert.equal(groups.length, 2)
})

test('cli reads stdin when no path is given', () => {
  const stdout = execFileSync('node', [script], { input: LOG, encoding: 'utf8' })
  assert.match(stdout, /### STT/)
})
