#!/usr/bin/env node
// Vendor latency bench CLI: runs crates/vendor-<id>/tests/live_bench.rs for the chosen vendors and
// writes raw.jsonl, summary.json, summary.md and environment.json.
//
//   node benchmarks/scripts/bench-vendors.mjs --vendors deepgram --runs 3 --region my-host \
//     --env-file ~/vendor-keys.env
//
// Flags: --vendors <list|all>  --exclude <list>  --runs <n>  --transport <all|websocket|http|auto|…>
//        --region <label>  --out <dir>  --env-file <path>  --require  --force
//        --scrub-logs <dir>   delete logs that contain a key value (lists them by name), then exit
//
// Keys are read from the process env, then the env file, by their library names
// (DEEPGRAM_API_KEY, …) or their CI secret names (DEEPGRAM_LIVE_KEY, …). A key value is never
// printed, logged or written: every output file is checked against the resolved values first.

import { spawn } from 'node:child_process'
import {
  closeSync,
  existsSync,
  mkdirSync,
  openSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  parseBenchLines,
  renderMarkdown,
  summarize,
  withDerivedMetrics,
} from '../../scripts/ci/vendor-bench-summary.mjs'
import { parseEnvFile, resolveVendorEnv } from './env-file.mjs'
import { collectEnvironment, startLoadSampler, startSuspendWatch } from './environment.mjs'
import { VENDORS, allVendorEnvNames } from './vendors.mjs'

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')
const TRANSPORTS = new Set(['all', 'websocket', 'http', 'auto', 'rest', 'grpc'])
const VALUE_FLAGS = new Set([
  '--vendors',
  '--exclude',
  '--runs',
  '--transport',
  '--region',
  '--out',
  '--env-file',
  '--scrub-logs',
])
const BOOL_FLAGS = new Set(['--require', '--force'])

class UsageError extends Error {}

export function parseArgs(argv) {
  const opts = {
    vendors: 'all',
    exclude: '',
    runs: 5,
    transport: 'all',
    region: 'unlabelled',
    out: null,
    envFile: null,
    require: false,
    force: false,
    scrubLogs: null,
  }
  for (let i = 0; i < argv.length; i += 1) {
    const flag = argv[i]
    if (BOOL_FLAGS.has(flag)) {
      opts[flag.slice(2)] = true
    } else if (VALUE_FLAGS.has(flag)) {
      const value = argv[i + 1]
      if (value === undefined || value.startsWith('--'))
        throw new UsageError(`${flag} needs a value`)
      i += 1
      const key = flag.slice(2).replace(/-([a-z])/g, (_, c) => c.toUpperCase())
      opts[key] = value
    } else {
      throw new UsageError(`unknown argument: ${flag}`)
    }
  }
  const ids = VENDORS.map((v) => v.id)
  const list = (text) =>
    text
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean)
  opts.selected = opts.vendors === 'all' ? ids : list(opts.vendors)
  opts.excluded = list(opts.exclude)
  for (const id of [...opts.selected, ...opts.excluded]) {
    if (!ids.includes(id)) throw new UsageError(`unknown vendor: ${id} (known: ${ids.join(', ')})`)
  }
  if (opts.selected.length === 0) throw new UsageError('--vendors is empty')
  const runs = Number(opts.runs)
  if (!Number.isInteger(runs) || runs < 1) throw new UsageError(`--runs must be a positive integer`)
  opts.runs = runs
  const transports = list(String(opts.transport))
  if (transports.length === 0 || transports.some((t) => !TRANSPORTS.has(t))) {
    throw new UsageError(`--transport must be ${[...TRANSPORTS].join(', ')} or a comma list`)
  }
  opts.transport = transports.join(',')
  return opts
}

function runProcess(command, args, { cwd, env, logPath }) {
  return new Promise((resolve) => {
    const fd = openSync(logPath, 'w')
    const child = spawn(command, args, { cwd, env, stdio: ['ignore', fd, fd] })
    child.once('error', () => {
      closeSync(fd)
      resolve(127)
    })
    child.once('close', (code, signal) => {
      closeSync(fd)
      resolve(code ?? (signal ? 128 : 1))
    })
  })
}

function stamp() {
  return new Date().toISOString().replace(/[-:]/g, '').replace(/\..*/, '').replace('T', '-')
}

function scrubLogs(dir, secrets) {
  const removed = []
  const walk = (d) => {
    for (const entry of readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, entry.name)
      if (entry.isDirectory()) walk(p)
      else if (secrets.some((s) => readFileSync(p, 'utf8').includes(s))) {
        rmSync(p)
        removed.push(path.relative(dir, p))
      }
    }
  }
  if (existsSync(dir)) walk(dir)
  return removed
}

export async function main(argv, processEnv = process.env) {
  let opts
  try {
    opts = parseArgs(argv)
  } catch (err) {
    process.stderr.write(`bench-vendors: ${err.message}\n`)
    process.stderr.write(
      'usage: bench-vendors.mjs [--vendors list|all] [--exclude list] [--runs n] [--transport t] [--region label] [--out dir] [--env-file path] [--require] [--force]\n',
    )
    return 2
  }

  let fileEnv = new Map()
  if (opts.envFile) {
    try {
      fileEnv = parseEnvFile(readFileSync(opts.envFile, 'utf8'))
    } catch {
      process.stderr.write(`bench-vendors: cannot read --env-file ${opts.envFile}\n`)
      return 2
    }
  }
  const sources = [(name) => processEnv[name], (name) => fileEnv.get(name)]
  const resolved = new Map(VENDORS.map((v) => [v.id, resolveVendorEnv(v, sources)]))
  // Every resolved value of length >= 8 must appear in no output file.
  const secrets = [
    ...new Set(
      [...resolved.values()].flatMap((r) => Object.values(r.env)).filter((v) => v.length >= 8),
    ),
  ]

  if (opts.scrubLogs) {
    const removed = scrubLogs(path.resolve(opts.scrubLogs), secrets)
    for (const name of removed) process.stdout.write(`key value found in log ${name}; deleted\n`)
    return removed.length > 0 ? 1 : 0
  }

  const date = new Date().toISOString().slice(0, 10)
  const outDir = path.resolve(
    opts.out ?? path.join(REPO_ROOT, 'benchmarks/results', `${date}-${opts.region}`),
  )
  if (existsSync(path.join(outDir, 'summary.json')) && !opts.force) {
    process.stderr.write(
      `bench-vendors: ${outDir} already has summary.json (use --force or another --out)\n`,
    )
    return 2
  }
  mkdirSync(outDir, { recursive: true })

  const cargo = processEnv.NWR_BENCH_CARGO || 'cargo'
  const logDir = path.join(REPO_ROOT, '.test-logs', 'bench-vendors', stamp())
  mkdirSync(logDir, { recursive: true })
  const baseEnv = { ...processEnv }
  for (const name of allVendorEnvNames()) delete baseEnv[name]

  const startedAt = new Date().toISOString()
  const records = []
  const vendorsRun = []
  const skipped = []
  const failed = []
  const starved = []
  const hostSuspended = []
  const perVendorLoad = {}

  for (const id of opts.selected) {
    const vendor = VENDORS.find((v) => v.id === id)
    if (opts.excluded.includes(id)) {
      skipped.push({ vendor: id, reason: 'excluded' })
      continue
    }
    const { env, missing, sets } = resolved.get(id)
    if (missing.length > 0) {
      if (opts.require)
        failed.push({ vendor: id, phase: 'env', reason: `no key: ${missing.join(', ')}` })
      else skipped.push({ vendor: id, reason: `no key: ${missing.join(', ')}` })
      continue
    }
    const childEnv = {
      ...baseEnv,
      ...env,
      NWR_BENCH_RUNS: String(opts.runs),
      NWR_BENCH_TRANSPORT: opts.transport,
    }
    for (const flagId of sets ?? [vendor.envId]) {
      childEnv[`${flagId.toUpperCase()}_LIVE_REQUIRED`] = '1'
    }
    const crate = `node-webrtc-rust-vendor-${vendor.id}`
    const cargoArgs = ['test', '-p', crate, '--features', 'live', '--test', 'live_bench']

    process.stderr.write(`bench-vendors: ${id}: build\n`)
    const buildLog = path.join(logDir, `${id}.build.log`)
    const buildCode = await runProcess(cargo, [...cargoArgs, '--no-run'], {
      cwd: REPO_ROOT,
      env: childEnv,
      logPath: buildLog,
    })
    if (buildCode !== 0) {
      failed.push({
        vendor: id,
        phase: 'build',
        exit_code: buildCode,
        log: path.relative(REPO_ROOT, buildLog),
      })
      continue
    }

    process.stderr.write(`bench-vendors: ${id}: run\n`)
    const runLog = path.join(logDir, `${id}.run.log`)
    const sampler = startLoadSampler()
    const watch = startSuspendWatch()
    const runCode = await runProcess(
      cargo,
      [...cargoArgs, '--', '--nocapture', '--test-threads=1'],
      {
        cwd: REPO_ROOT,
        env: childEnv,
        logPath: runLog,
      },
    )
    const load = sampler.stop()
    const gaps = watch.stop()
    perVendorLoad[id] = load
    if (load.starved) starved.push(id)
    for (const gap of gaps) hostSuspended.push({ vendor: id, ...gap })
    vendorsRun.push(id)
    records.push(...parseBenchLines(readFileSync(runLog, 'utf8')))
    if (runCode !== 0) {
      failed.push({
        vendor: id,
        phase: 'run',
        exit_code: runCode,
        log: path.relative(REPO_ROOT, runLog),
      })
    }
  }

  const finishedAt = new Date().toISOString()
  let scrubFailed = false
  for (const name of scrubLogs(logDir, secrets)) {
    process.stderr.write(`key value found in log ${name}; deleted\n`)
    scrubFailed = true
  }
  const groups = summarize(records.map(withDerivedMetrics))
  const runVendors = VENDORS.filter((v) => vendorsRun.includes(v.id))
  const environment = await collectEnvironment({
    region: opts.region,
    repoRoot: REPO_ROOT,
    vendors: runVendors,
    lookupEnv: (name) => runVendors.map((v) => resolved.get(v.id).env[name]).find(Boolean),
    probes: processEnv.NWR_BENCH_NO_PROBE === '1' ? null : undefined,
    startedAt,
    finishedAt,
  })
  const summary = {
    schema_version: 1,
    generated_at: finishedAt,
    region: opts.region,
    runs: opts.runs,
    transport: opts.transport,
    vendors_run: vendorsRun,
    skipped,
    failed,
    starved,
    host_suspended: hostSuspended,
    per_vendor_load: perVendorLoad,
    groups,
  }

  const cpu = `${environment.cpu_model ?? 'unknown cpu'} x${environment.cpu_cores}`
  const osText = `${environment.os.platform} ${environment.os.release} ${environment.os.arch}`
  const md = [
    `# Vendor latency bench`,
    '',
    `- Date: ${finishedAt.slice(0, 10)}`,
    `- Region: ${opts.region}`,
    `- nwr ${environment.nwr_version ?? 'unknown'} (${(environment.git_sha ?? 'unknown').slice(0, 9)}${environment.git_dirty ? ', dirty' : ''})`,
    `- Host: ${osText}, ${cpu}`,
    `- Runs: ${opts.runs}, transport: ${opts.transport}`,
    '',
  ]
  const list = (title, items) => {
    if (items.length > 0) md.push(`**${title}:** ${items.join('; ')}`, '')
  }
  list(
    'Skipped',
    skipped.map((s) => `${s.vendor} (${s.reason})`),
  )
  list(
    'Failed',
    failed.map(
      (f) =>
        `${f.vendor} (${f.phase}${f.exit_code !== undefined ? ` exit ${f.exit_code}` : ''}${f.reason ? `, ${f.reason}` : ''})`,
    ),
  )
  list('Starved host (CPU busy p90 above 85%)', starved)
  list(
    'Host suspended',
    hostSuspended.map((h) => `${h.vendor} at ${h.at} (${h.gap_ms} ms)`),
  )
  if (hostSuspended.length > 0) {
    md.push('[host-suspended] result: inconclusive', '')
    process.stderr.write('[host-suspended] result: inconclusive\n')
  }
  md.push(renderMarkdown(groups))

  const write = (name, content) => {
    if (secrets.some((s) => content.includes(s))) {
      process.stderr.write(`key value found in ${name}; not written\n`)
      scrubFailed = true
      return
    }
    writeFileSync(path.join(outDir, name), content)
  }
  write(
    'raw.jsonl',
    records.map((r) => JSON.stringify(r)).join('\n') + (records.length ? '\n' : ''),
  )
  write('environment.json', `${JSON.stringify(environment, null, 2)}\n`)
  write('summary.json', `${JSON.stringify(summary, null, 2)}\n`)
  write('summary.md', `${md.join('\n')}\n`)
  process.stderr.write(`bench-vendors: results in ${outDir}\n`)
  return failed.length > 0 || scrubFailed ? 1 : 0
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).then((code) => process.exit(code))
}
