import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import {
  chmodSync,
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const cli = path.join(path.dirname(fileURLToPath(import.meta.url)), 'bench-vendors.mjs')
const KEY = 'dg-test-key-0123456789'
const OTHER_KEY = 'groq-test-key-9876543210'

// A stand-in for cargo. `--no-run` is the build; anything else is the run. It records the calls
// and the NAMES of its environment (never values of keys) in $FAKE_DIR.
const FAKE_CARGO = `#!/bin/sh
crate="\${3##*vendor-}"
echo "$crate $*" >> "$FAKE_DIR/calls"
touch "$FAKE_DIR/marker"
case "$*" in
  *--no-run*) exit "\${FAKE_BUILD_EXIT:-0}" ;;
esac
env | cut -d= -f1 | sort > "$FAKE_DIR/envnames-$crate"
env | grep -E '^(NWR_BENCH_|[A-Z_]+_LIVE_REQUIRED=)' | sort > "$FAKE_DIR/flags-$crate"
echo "running 1 test"
model=m
if [ -n "$FAKE_ECHO_KEY" ]; then model="$DEEPGRAM_API_KEY"; fi
for run in 0 1; do
  echo "vendor_bench {\\"vendor\\":\\"$crate\\",\\"leg\\":\\"tts\\",\\"transport\\":\\"http\\",\\"model\\":\\"$model\\",\\"phase\\":\\"cold\\",\\"text\\":\\"short\\",\\"run\\":$run,\\"http_version\\":null,\\"metrics\\":{\\"ttfa_ms\\":$((100 + run)),\\"total_ms\\":400,\\"audio_ms\\":2000}}"
  echo "noise line $run"
done
if [ -n "$FAKE_NOISE_KEY" ]; then echo "debug $DEEPGRAM_API_KEY"; fi
if [ "$crate" = "$FAKE_FAIL_CRATE" ]; then exit 3; fi
exit 0
`

function setup() {
  const dir = mkdtempSync(path.join(tmpdir(), 'nwr-bench-test-'))
  const fake = path.join(dir, 'fake-cargo.sh')
  writeFileSync(fake, FAKE_CARGO)
  chmodSync(fake, 0o755)
  const out = path.join(dir, 'out')
  const run = (args, env = {}) => {
    const result = spawnSync(process.execPath, [cli, ...args, '--out', out, '--region', 'test'], {
      env: {
        PATH: process.env.PATH,
        NWR_BENCH_CARGO: fake,
        FAKE_DIR: dir,
        NWR_BENCH_NO_PROBE: '1',
        ...env,
      },
      encoding: 'utf8',
    })
    return result
  }
  const json = (name) => JSON.parse(readFileSync(path.join(out, name), 'utf8'))
  return { dir, out, run, json }
}

test('without keys every vendor is skipped and cargo never runs', () => {
  const { dir, out, run, json } = setup()
  const result = run([])
  assert.equal(result.status, 0, result.stderr)
  assert.equal(existsSync(path.join(dir, 'marker')), false)
  const summary = json('summary.json')
  assert.equal(summary.skipped.length, 10)
  for (const s of summary.skipped) {
    const unpublished = ['deepgram', 'assemblyai', 'google', 'azure', 'aws'].includes(s.vendor)
    assert.match(s.reason, unpublished ? /^not published: / : /^no key: /)
  }
  assert.deepEqual(summary.vendors_run, [])
  assert.ok(existsSync(path.join(out, 'summary.md')))
  assert.ok(existsSync(path.join(out, 'environment.json')))
})

test('one vendor with a key: records, summary and environment are written', () => {
  const { dir, out, run, json } = setup()
  const result = run(['--vendors', 'deepgram', '--runs', '2'], {
    DEEPGRAM_API_KEY: KEY,
    GROQ_API_KEY: OTHER_KEY,
  })
  assert.equal(result.status, 0, result.stderr)
  const raw = readFileSync(path.join(out, 'raw.jsonl'), 'utf8').trim().split('\n')
  assert.equal(raw.length, 2)
  const summary = json('summary.json')
  assert.deepEqual(summary.vendors_run, ['deepgram'])
  assert.equal(summary.groups.length, 1)
  assert.equal(summary.groups[0].metrics.real_time_factor.median, 0.2)
  assert.ok(summary.per_vendor_load.deepgram)
  assert.equal(json('environment.json').region, 'test')
  for (const name of readdirSync(out)) {
    const text = readFileSync(path.join(out, name), 'utf8')
    assert.equal(text.includes(KEY), false, `${name} holds the key`)
    assert.equal(text.includes(OTHER_KEY), false, `${name} holds another key`)
  }
  const names = readFileSync(path.join(dir, 'envnames-deepgram'), 'utf8').split('\n')
  assert.ok(names.includes('DEEPGRAM_API_KEY'))
  assert.equal(names.includes('GROQ_API_KEY'), false, 'another vendor var reached the child')
  const flags = readFileSync(path.join(dir, 'flags-deepgram'), 'utf8')
  assert.match(flags, /^NWR_BENCH_RUNS=2$/m)
  assert.match(flags, /^NWR_BENCH_TRANSPORT=all$/m)
  assert.match(flags, /^DEEPGRAM_LIVE_REQUIRED=1$/m)
  const calls = readFileSync(path.join(dir, 'calls'), 'utf8').trim().split('\n')
  assert.equal(calls.length, 2)
  assert.match(calls[0], /--no-run/)
  assert.match(calls[1], /--nocapture --test-threads=1/)
})

test('a failing vendor is listed under failed and the next vendor still runs', () => {
  const { out, run, json } = setup()
  const result = run(['--vendors', 'deepgram,groq'], {
    DEEPGRAM_API_KEY: KEY,
    GROQ_API_KEY: OTHER_KEY,
    FAKE_FAIL_CRATE: 'deepgram',
  })
  assert.equal(result.status, 1)
  const summary = json('summary.json')
  assert.equal(summary.failed.length, 1)
  assert.equal(summary.failed[0].vendor, 'deepgram')
  assert.equal(summary.failed[0].phase, 'run')
  assert.equal(summary.failed[0].exit_code, 3)
  assert.deepEqual(summary.vendors_run.sort(), ['deepgram', 'groq'])
  const raw = readFileSync(path.join(out, 'raw.jsonl'), 'utf8')
  assert.match(raw, /"vendor":"groq"/)
  assert.match(raw, /"vendor":"deepgram"/, 'records of a failed run are kept')
})

test('a failing build is listed under failed with phase build', () => {
  const { run, json } = setup()
  const result = run(['--vendors', 'deepgram'], { DEEPGRAM_API_KEY: KEY, FAKE_BUILD_EXIT: '101' })
  assert.equal(result.status, 1)
  const failed = json('summary.json').failed
  assert.equal(failed[0].phase, 'build')
  assert.equal(failed[0].exit_code, 101)
})

test('a key value echoed by cargo is never written', () => {
  const { out, run } = setup()
  const result = run(['--vendors', 'deepgram'], { DEEPGRAM_API_KEY: KEY, FAKE_ECHO_KEY: '1' })
  assert.equal(result.status, 1)
  assert.equal(existsSync(path.join(out, 'raw.jsonl')), false)
  assert.match(result.stderr, /key value found in raw\.jsonl; not written/)
  assert.equal(result.stderr.includes(KEY), false)
  assert.equal(result.stdout.includes(KEY), false)
  for (const name of readdirSync(out)) {
    assert.equal(readFileSync(path.join(out, name), 'utf8').includes(KEY), false, name)
  }
})

test('--require with a missing key fails and names the vendor', () => {
  const { dir, run, json } = setup()
  const result = run(['--vendors', 'deepgram', '--require'])
  assert.equal(result.status, 1)
  const failed = json('summary.json').failed
  assert.equal(failed.length, 1)
  assert.equal(failed[0].vendor, 'deepgram')
  assert.equal(existsSync(path.join(dir, 'marker')), false)
})

test('--exclude skips a vendor, secret names work as aliases, and --transport is passed on', () => {
  const { dir, run, json } = setup()
  const result = run(['--vendors', 'deepgram,groq', '--exclude', 'groq', '--transport', 'http'], {
    DEEPGRAM_LIVE_KEY: KEY,
    GROQ_API_KEY: OTHER_KEY,
  })
  assert.equal(result.status, 0, result.stderr)
  const summary = json('summary.json')
  assert.deepEqual(summary.skipped, [{ vendor: 'groq', reason: 'excluded' }])
  assert.deepEqual(summary.vendors_run, ['deepgram'])
  assert.match(
    readFileSync(path.join(dir, 'flags-deepgram'), 'utf8'),
    /^NWR_BENCH_TRANSPORT=http$/m,
  )
})

test('usage errors exit 2 and an existing summary.json needs --force', () => {
  const { run } = setup()
  assert.equal(run(['--vendors', 'nope']).status, 2)
  assert.equal(run(['--bogus']).status, 2)
  assert.equal(run(['--runs', '0']).status, 2)
  assert.equal(run(['--transport', 'carrier-pigeon']).status, 2)
  assert.equal(run([]).status, 0)
  assert.equal(run([]).status, 2, 'second run into the same folder')
  assert.equal(run(['--force']).status, 0)
})

test('a key value in a cargo log is deleted locally', () => {
  const { out, run } = setup()
  const result = run(['--vendors', 'deepgram'], { DEEPGRAM_API_KEY: KEY, FAKE_NOISE_KEY: '1' })
  assert.equal(result.status, 1)
  assert.match(result.stderr, /key value found in log deepgram\.run\.log; deleted/)
  assert.equal(result.stderr.includes(KEY), false)
  assert.equal(readFileSync(path.join(out, 'raw.jsonl'), 'utf8').includes(KEY), false)
  const logRoot = path.join(path.dirname(cli), '../../.test-logs/bench-vendors')
  const stamps = readdirSync(logRoot).sort()
  const newest = path.join(logRoot, stamps[stamps.length - 1])
  assert.equal(existsSync(path.join(newest, 'deepgram.run.log')), false)
})

test('--vendors all skips unpublished vendors with their reason even when keys are set', () => {
  const { dir, run, json } = setup()
  const result = run([], {
    DEEPGRAM_API_KEY: KEY,
    ASSEMBLYAI_API_KEY: OTHER_KEY,
    GROQ_API_KEY: 'groq-test-key-5555555555',
  })
  assert.equal(result.status, 0, result.stderr)
  const summary = json('summary.json')
  assert.deepEqual(summary.vendors_run, ['groq'])
  for (const id of ['deepgram', 'assemblyai']) {
    const entry = summary.skipped.find((s) => s.vendor === id)
    assert.match(entry.reason, /^not published: vendor terms do not allow benchmarking$/)
  }
  const calls = readFileSync(path.join(dir, 'calls'), 'utf8')
  assert.equal(/^(deepgram|assemblyai) /m.test(calls), false, 'cargo ran for an unpublished vendor')
  assert.ok(existsSync(path.join(dir, 'envnames-groq')))
  assert.equal(existsSync(path.join(dir, 'envnames-deepgram')), false)
  assert.equal(existsSync(path.join(dir, 'envnames-assemblyai')), false)
})

test('an unpublished vendor named explicitly runs', () => {
  const { dir, run, json } = setup()
  const result = run(['--vendors', 'deepgram'], { DEEPGRAM_API_KEY: KEY })
  assert.equal(result.status, 0, result.stderr)
  assert.deepEqual(json('summary.json').vendors_run, ['deepgram'])
  assert.match(readFileSync(path.join(dir, 'calls'), 'utf8'), /^deepgram /m)
})
