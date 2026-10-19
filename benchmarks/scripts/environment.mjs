// Records where and on what a bench ran: build identity, host, network path to each vendor host,
// host load while a vendor ran, and host suspends (a suspended host makes a run inconclusive).

import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import dns from 'node:dns/promises'
import net from 'node:net'
import os from 'node:os'
import path from 'node:path'
import { performance } from 'node:perf_hooks'
import { clearInterval, clearTimeout, setInterval, setTimeout } from 'node:timers'
import tls from 'node:tls'

/** A host is starved when its CPU busy p90 over a vendor window exceeds this percent. */
export const STARVED_BUSY_P90 = 85
/** A gap between 1 s ticks above this many ms is a suspend. */
export const SUSPEND_GAP_MS = 5000
const PROBE_TIMEOUT_MS = 5000
const PROBE_SAMPLES = 3

function median(values) {
  if (values.length === 0) return null
  const sorted = [...values].sort((a, b) => a - b)
  const mid = sorted.length >> 1
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2
}

function percentile(values, p) {
  if (values.length === 0) return null
  const sorted = [...values].sort((a, b) => a - b)
  return sorted[Math.max(0, Math.ceil((p / 100) * sorted.length) - 1)]
}

const round1 = (n) => (n === null ? null : Math.round(n * 10) / 10)

// --- network probes -------------------------------------------------------------------------

function withTimeout(promise, ms) {
  let timer
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(Object.assign(new Error('timeout'), { code: 'ETIMEDOUT' })), ms)
  })
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer))
}

/** Default probes: DNS lookup, TCP connect to 443, TLS handshake. No HTTP request is sent. */
export const defaultProbes = {
  async dns(host) {
    const start = performance.now()
    const result = await withTimeout(dns.lookup(host), PROBE_TIMEOUT_MS)
    return { ms: performance.now() - start, family: result.family === 6 ? 'IPv6' : 'IPv4' }
  },
  tcp(host) {
    return new Promise((resolve, reject) => {
      const start = performance.now()
      const socket = net.connect({ host, port: 443 })
      const fail = (err) => {
        socket.destroy()
        reject(err)
      }
      socket.setTimeout(PROBE_TIMEOUT_MS, () =>
        fail(Object.assign(new Error('timeout'), { code: 'ETIMEDOUT' })),
      )
      socket.once('error', fail)
      socket.once('connect', () => {
        const ms = performance.now() - start
        socket.destroy()
        resolve({ ms })
      })
    })
  },
  tls(host) {
    return new Promise((resolve, reject) => {
      const start = performance.now()
      const socket = tls.connect({
        host,
        port: 443,
        servername: host,
        ALPNProtocols: ['h2', 'http/1.1'],
      })
      const fail = (err) => {
        socket.destroy()
        reject(err)
      }
      socket.setTimeout(PROBE_TIMEOUT_MS, () =>
        fail(Object.assign(new Error('timeout'), { code: 'ETIMEDOUT' })),
      )
      socket.once('error', fail)
      socket.once('secureConnect', () => {
        const ms = performance.now() - start
        const alpn = socket.alpnProtocol || null
        socket.destroy()
        resolve({ ms, alpn })
      })
    })
  },
}

/** Median of 3 DNS / TCP / TLS samples for one host. A failing probe records `error: <code>`. */
export async function probeHost(host, probes = defaultProbes, samples = PROBE_SAMPLES) {
  const out = { host, dns_ms: null, tcp_ms: null, tls_ms: null, alpn: null, address_family: null }
  const errors = []
  for (const [key, probe] of [
    ['dns_ms', probes.dns],
    ['tcp_ms', probes.tcp],
    ['tls_ms', probes.tls],
  ]) {
    const values = []
    for (let i = 0; i < samples; i += 1) {
      try {
        const r = await probe(host)
        values.push(r.ms)
        if (r.alpn) out.alpn = r.alpn
        if (r.family) out.address_family = r.family
      } catch (err) {
        errors.push(String(err?.code || err?.name || 'ERROR'))
      }
    }
    out[key] = round1(median(values))
  }
  if (errors.length > 0) out.error = errors[0]
  return out
}

/** Hosts of a vendor with `{region}` filled; templated hosts without a region are skipped. */
export function vendorHosts(vendor, lookupEnv) {
  const hosts = []
  for (const host of vendor.hosts) {
    if (host.includes('{region}')) {
      const region = vendor.regionVar ? lookupEnv(vendor.regionVar) : undefined
      if (!region) continue
      hosts.push(host.replace('{region}', region))
    } else {
      hosts.push(host)
    }
  }
  return hosts
}

// --- host load and suspend ------------------------------------------------------------------

/** Busy percent between two `os.cpus()` snapshots. */
export function cpuBusyPercent(prev, cur) {
  let busy = 0
  let total = 0
  for (let i = 0; i < cur.length; i += 1) {
    const a = prev[i]?.times
    const b = cur[i].times
    if (!a) continue
    const idle = b.idle - a.idle
    const sum = Object.keys(b).reduce((acc, k) => acc + (b[k] - (a[k] ?? 0)), 0)
    total += sum
    busy += sum - idle
  }
  return total > 0 ? (busy / total) * 100 : 0
}

/** Statistics of load samples `{ busy, load1 }`. */
export function summarizeLoad(samples) {
  const busy = samples.map((s) => s.busy)
  const p90 = percentile(busy, 90)
  return {
    samples: samples.length,
    busy_median: round1(median(busy)),
    busy_p90: round1(p90),
    busy_max: round1(busy.length ? Math.max(...busy) : null),
    load1_max: round1(samples.length ? Math.max(...samples.map((s) => s.load1)) : null),
    starved: p90 !== null && p90 > STARVED_BUSY_P90,
  }
}

/**
 * Samples the host CPU busy percent and 1-minute load every second. `stop()` returns the stats.
 * `deps` are injectable for tests; `tick()` takes one sample by hand.
 */
export function startLoadSampler(deps = {}) {
  const cpus = deps.cpus ?? os.cpus
  const loadavg = deps.loadavg ?? os.loadavg
  const setTimer = deps.setInterval ?? setInterval
  const clearTimer = deps.clearInterval ?? clearInterval
  const samples = []
  let prev = cpus()
  const tick = () => {
    const cur = cpus()
    samples.push({ busy: cpuBusyPercent(prev, cur), load1: loadavg()[0] })
    prev = cur
  }
  const timer = setTimer(tick, 1000)
  timer.unref?.()
  return {
    tick,
    stop() {
      clearTimer(timer)
      return summarizeLoad(samples)
    },
  }
}

/** Records every gap above 5 s between 1 s ticks: the host was suspended. */
export function startSuspendWatch(deps = {}) {
  const now = deps.now ?? Date.now
  const setTimer = deps.setInterval ?? setInterval
  const clearTimer = deps.clearInterval ?? clearInterval
  const gaps = []
  let last = now()
  const tick = () => {
    const t = now()
    if (t - last > SUSPEND_GAP_MS) gaps.push({ at: new Date(t).toISOString(), gap_ms: t - last })
    last = t
  }
  const timer = setTimer(tick, 1000)
  timer.unref?.()
  return {
    tick,
    stop() {
      clearTimer(timer)
      return gaps
    },
  }
}

// --- environment record ---------------------------------------------------------------------

function sh(cmd, args, cwd) {
  try {
    return execFileSync(cmd, args, {
      cwd,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim()
  } catch {
    return null
  }
}

/** Released nwr version: `version` of packages/sdk/package.json (null if unreadable). */
export function readNwrVersion(repoRoot) {
  try {
    const pkg = JSON.parse(readFileSync(path.join(repoRoot, 'packages/sdk/package.json'), 'utf8'))
    return typeof pkg.version === 'string' ? pkg.version : null
  } catch {
    return null
  }
}

/**
 * The `environment.json` record. `probes: null` skips the network probes (`endpoints` is `[]`).
 * @param {{ region?: string, repoRoot: string, vendors: any[], lookupEnv?: (n: string) => string | undefined, probes?: object | null, startedAt?: string, finishedAt?: string }} opts
 */
export async function collectEnvironment(opts) {
  const { repoRoot, vendors } = opts
  const lookupEnv = opts.lookupEnv ?? ((name) => process.env[name])
  const cpuList = os.cpus()
  const endpoints = []
  if (opts.probes !== null) {
    for (const vendor of vendors) {
      for (const host of vendorHosts(vendor, lookupEnv)) {
        endpoints.push({
          vendor: vendor.id,
          ...(await probeHost(host, opts.probes ?? defaultProbes)),
        })
      }
    }
  }
  const status = sh('git', ['status', '--porcelain'], repoRoot)
  return {
    nwr_version: readNwrVersion(repoRoot),
    git_sha: sh('git', ['rev-parse', 'HEAD'], repoRoot),
    git_dirty: status === null ? null : status.length > 0,
    os: { platform: os.platform(), release: os.release(), arch: os.arch() },
    cpu_model: cpuList[0]?.model ?? null,
    cpu_cores: cpuList.length,
    memory_gb: Math.round((os.totalmem() / 1024 ** 3) * 10) / 10,
    node_version: process.version,
    rustc_version: sh('rustc', ['--version'], repoRoot),
    region: opts.region || 'unlabelled',
    started_at: opts.startedAt ?? null,
    finished_at: opts.finishedAt ?? null,
    endpoints,
  }
}
