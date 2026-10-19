import assert from 'node:assert/strict'
import test from 'node:test'

import {
  STARVED_BUSY_P90,
  cpuBusyPercent,
  probeHost,
  startLoadSampler,
  startSuspendWatch,
  summarizeLoad,
  vendorHosts,
} from './environment.mjs'

function sequence(values) {
  let i = 0
  return async () => {
    const v = values[Math.min(i, values.length - 1)]
    i += 1
    if (v instanceof Error) throw v
    return v
  }
}

test('probeHost records the median of three samples and the negotiated alpn', async () => {
  const probes = {
    dns: sequence([{ ms: 3, family: 'IPv4' }, { ms: 1 }, { ms: 2 }]),
    tcp: sequence([{ ms: 10 }, { ms: 30 }, { ms: 20 }]),
    tls: sequence([
      { ms: 40, alpn: 'h2' },
      { ms: 50, alpn: 'h2' },
      { ms: 60, alpn: 'h2' },
    ]),
  }
  const r = await probeHost('example.test', probes)
  assert.deepEqual(r, {
    host: 'example.test',
    dns_ms: 2,
    tcp_ms: 20,
    tls_ms: 50,
    alpn: 'h2',
    address_family: 'IPv4',
  })
})

test('a probe error is recorded as its code and never thrown', async () => {
  const notFound = Object.assign(new Error('x'), { code: 'ENOTFOUND' })
  const probes = { dns: sequence([notFound]), tcp: sequence([notFound]), tls: sequence([notFound]) }
  const r = await probeHost('nope.test', probes)
  assert.equal(r.error, 'ENOTFOUND')
  assert.equal(r.dns_ms, null)
  assert.equal(r.tls_ms, null)
})

test('a probe timeout is recorded as ETIMEDOUT and the other samples still count', async () => {
  const timeout = Object.assign(new Error('timeout'), { code: 'ETIMEDOUT' })
  const probes = {
    dns: sequence([{ ms: 5 }]),
    tcp: sequence([timeout, { ms: 8 }, { ms: 12 }]),
    tls: sequence([{ ms: 20 }]),
  }
  const r = await probeHost('slow.test', probes)
  assert.equal(r.error, 'ETIMEDOUT')
  assert.equal(r.tcp_ms, 10)
})

test('templated hosts need their region variable', () => {
  const vendor = { hosts: ['plain.test', '{region}.t.test'], regionVar: 'R' }
  assert.deepEqual(
    vendorHosts(vendor, () => undefined),
    ['plain.test'],
  )
  assert.deepEqual(
    vendorHosts(vendor, () => 'eu1'),
    ['plain.test', 'eu1.t.test'],
  )
})

const snap = (idle, busy) => [{ times: { user: busy, nice: 0, sys: 0, idle, irq: 0 } }]

test('cpuBusyPercent is the busy share of the delta', () => {
  assert.equal(cpuBusyPercent(snap(0, 0), snap(25, 75)), 75)
  assert.equal(cpuBusyPercent(snap(10, 10), snap(10, 10)), 0)
})

test('load sampler reports median, p90, max, load1 max and starved', () => {
  let t = 0
  let load = 1
  const cpus = () => snap(t * 10, t * 90) // 90 percent busy on every tick
  let timerFn
  const sampler = startLoadSampler({
    cpus,
    loadavg: () => [load, 0, 0],
    setInterval: (fn) => {
      timerFn = fn
      return {}
    },
    clearInterval: () => {},
  })
  for (let i = 1; i <= 10; i += 1) {
    t = i
    load = i
    timerFn()
  }
  const stats = sampler.stop()
  assert.equal(stats.samples, 10)
  assert.equal(stats.busy_median, 90)
  assert.equal(stats.busy_p90, 90)
  assert.equal(stats.busy_max, 90)
  assert.equal(stats.load1_max, 10)
  assert.equal(stats.starved, true)
  assert.ok(STARVED_BUSY_P90 < 90)
})

test('an idle host is not starved', () => {
  const stats = summarizeLoad([
    { busy: 10, load1: 0.5 },
    { busy: 20, load1: 0.7 },
  ])
  assert.equal(stats.starved, false)
  assert.equal(stats.busy_max, 20)
})

test('suspend watch records a gap above 5 s from an injected clock', () => {
  let now = 1_000_000
  let tick
  const watch = startSuspendWatch({
    now: () => now,
    setInterval: (fn) => {
      tick = fn
      return {}
    },
    clearInterval: () => {},
  })
  now += 1000
  tick()
  now += 203_000
  tick()
  now += 1000
  tick()
  const gaps = watch.stop()
  assert.equal(gaps.length, 1)
  assert.equal(gaps[0].gap_ms, 203_000)
  assert.equal(gaps[0].at, new Date(1_000_000 + 204_000).toISOString())
})
