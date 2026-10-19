// Guards benchmarks/METHODOLOGY.md against drift: every metric the Rust samples serialize, every
// record field, and every top-level key of summary.json and environment.json must be documented,
// and the public docs must not point at private repos.

import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(here, '../..')
const cli = path.join(here, 'bench-vendors.mjs')
const read = (rel) => readFileSync(path.join(repoRoot, rel), 'utf8')

/**
 * Serialized field names of `pub struct <name> { ... }` in Rust source text: the `pub <field>:`
 * lines, with `#[serde(rename = "x")]` applied and `#[serde(skip...)]` fields left out.
 */
export function structFields(source, name) {
  const lines = source.split('\n')
  const start = lines.findIndex((l) => new RegExp(`^pub struct ${name}\\s*\\{`).test(l))
  assert.notEqual(start, -1, `pub struct ${name} not found`)
  const fields = []
  let attrs = []
  for (let i = start + 1; i < lines.length; i += 1) {
    const line = lines[i].trim()
    if (line.startsWith('}')) return fields
    if (line.startsWith('///') || line === '') continue
    if (line.startsWith('#[')) {
      attrs.push(line)
      continue
    }
    const field = /^pub\s+(?:r#)?([A-Za-z0-9_]+)\s*:/.exec(line)
    if (field) {
      const serde = attrs.filter((a) => a.startsWith('#[serde('))
      const skipped = serde.some((a) => /[(,\s]skip(?:_serializing\w*)?\b/.test(a))
      const rename = serde.map((a) => /rename\s*=\s*"([^"]+)"/.exec(a)).find(Boolean)
      if (!skipped) fields.push(rename ? rename[1] : field[1])
    }
    attrs = []
  }
  assert.fail(`pub struct ${name} is not closed`)
}

/** Names of `names` that are not the first cell (`| `name` ...`) of a markdown table row. */
export function undocumented(names, markdown) {
  const cells = new Set()
  for (const line of markdown.split('\n')) {
    const m = /^\|\s*`([A-Za-z0-9_]+)`/.exec(line)
    if (m) cells.add(m[1])
  }
  return names.filter((n) => !cells.has(n))
}

/** The section of `## <heading>` up to the next `## ` heading. */
function section(markdown, heading) {
  const start = markdown.indexOf(`\n## ${heading}`)
  assert.notEqual(start, -1, `section "${heading}" missing`)
  const next = markdown.indexOf('\n## ', start + 1)
  return markdown.slice(start, next === -1 ? undefined : next)
}

const FIXTURE = `
/// doc
#[derive(Debug)]
pub struct Sample {
    /// first
    pub alpha_ms: u64,
    #[serde(rename = "beta_ms")]
    pub gamma: u64,
    #[serde(skip)]
    pub hidden: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optional: Option<u64>,
    pub delta: f64,
}

pub struct Other {
    pub not_mine: u8,
}
`

test('structFields reads names, honours rename and skip, and stops at the struct end', () => {
  assert.deepEqual(structFields(FIXTURE, 'Sample'), ['alpha_ms', 'beta_ms', 'delta'])
  assert.deepEqual(structFields(FIXTURE, 'Other'), ['not_mine'])
})

test('a field added to a struct is reported when the doc lacks it', () => {
  const doc = '| `alpha_ms` | ms |\n| `beta_ms` | ms |\n| `delta` | ratio |\n'
  assert.deepEqual(undocumented(structFields(FIXTURE, 'Sample'), doc), [])
  const grown = FIXTURE.replace('pub delta: f64,', 'pub delta: f64,\n    pub epsilon: u64,')
  assert.deepEqual(undocumented(structFields(grown, 'Sample'), doc), ['epsilon'])
  assert.deepEqual(undocumented(['alpha_ms'], 'prose with `alpha_ms` outside a table'), [
    'alpha_ms',
  ])
})

test('the real sample structs parse to the known fields', () => {
  const tts = structFields(read('crates/vendor-live-support/src/tts.rs'), 'TtsSample')
  const stt = structFields(read('crates/vendor-live-support/src/stt.rs'), 'SttSample')
  const record = structFields(read('crates/vendor-live-support/src/bench.rs'), 'BenchRecord')
  assert.ok(tts.includes('ttfa_ms') && tts.includes('first_before_return'))
  assert.ok(stt.includes('open_ms') && stt.includes('speech_end_to_final_ms'))
  assert.ok(record.includes('http_version') && record.includes('metrics'))
})

test('METHODOLOGY.md documents every metric and record field in its metric reference', () => {
  const names = [
    ...structFields(read('crates/vendor-live-support/src/tts.rs'), 'TtsSample'),
    ...structFields(read('crates/vendor-live-support/src/stt.rs'), 'SttSample'),
    'real_time_factor',
    ...structFields(read('crates/vendor-live-support/src/bench.rs'), 'BenchRecord'),
  ]
  const reference = section(read('benchmarks/METHODOLOGY.md'), 'Metric reference')
  const missing = undocumented([...new Set(names)], reference)
  assert.deepEqual(missing, [], `not in the METHODOLOGY.md metric reference: ${missing.join(', ')}`)
})

// A stand-in for cargo that prints one TTS and one STT sample for the crate it is asked for.
const FAKE_CARGO = `#!/bin/sh
case "$*" in *--no-run*) exit 0 ;; esac
echo 'vendor_bench {"vendor":"deepgram","leg":"tts","transport":"websocket","model":"m","phase":"cold","text":"short","run":0,"http_version":"ws","metrics":{"ttfa_ms":100,"total_ms":400,"audio_ms":2000}}'
echo 'vendor_bench {"vendor":"deepgram","leg":"stt","transport":"websocket","model":"m","phase":"cold","text":"phrase","run":0,"http_version":null,"metrics":{"open_ms":50,"first_partial_ms":null,"speech_end_to_final_ms":1700}}'
exit 0
`

test('METHODOLOGY.md documents every top-level key of summary.json and environment.json', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'nwr-methodology-test-'))
  const fake = path.join(dir, 'fake-cargo.sh')
  writeFileSync(fake, FAKE_CARGO)
  chmodSync(fake, 0o755)
  const out = path.join(dir, 'out')
  const result = spawnSync(
    process.execPath,
    [cli, '--vendors', 'deepgram', '--runs', '1', '--out', out, '--region', 'test'],
    {
      env: {
        PATH: process.env.PATH,
        NWR_BENCH_CARGO: fake,
        NWR_BENCH_NO_PROBE: '1',
        DEEPGRAM_API_KEY: 'methodology-test-key-0123456789',
      },
      encoding: 'utf8',
    },
  )
  assert.equal(result.status, 0, result.stderr)
  const summary = JSON.parse(readFileSync(path.join(out, 'summary.json'), 'utf8'))
  const environment = JSON.parse(readFileSync(path.join(out, 'environment.json'), 'utf8'))
  assert.ok(Object.keys(summary).length >= 12 && Object.keys(environment).length >= 13)
  const doc = read('benchmarks/METHODOLOGY.md')
  const missingSummary = undocumented(Object.keys(summary), doc)
  const missingEnv = undocumented(Object.keys(environment), doc)
  assert.deepEqual(missingSummary, [], `summary.json keys not documented: ${missingSummary}`)
  assert.deepEqual(missingEnv, [], `environment.json keys not documented: ${missingEnv}`)
})

test('the public benchmark docs link no private repo and name no internal path', () => {
  for (const file of ['benchmarks/METHODOLOGY.md', 'benchmarks/README.md']) {
    const text = read(file)
    for (const m of text.matchAll(/github\.com\/voicethere\/([A-Za-z0-9._-]+)/g)) {
      assert.ok(['client', 'agent', 'cli'].includes(m[1]), `${file} links private repo ${m[0]}`)
    }
    assert.doesNotMatch(text, /development\//, `${file} mentions development/`)
    assert.doesNotMatch(text, /webrtc-cloud/, `${file} mentions webrtc-cloud`)
  }
})
