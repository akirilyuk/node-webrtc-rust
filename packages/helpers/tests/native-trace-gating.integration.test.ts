/**
 * Runs the real native addon in a child process to prove the `WNR_LOG_FILE` gating
 * (guard for `net.connect-stall-no-timeout`, wiring in `native-trace-ci-wiring.test.ts`):
 * - unset: no file is created, even with RUST_LOG set (zero cost for normal runs);
 * - set: one file per process (`{pid}`), only transport targets, nothing from our own crates;
 * - set with a small WNR_LOG_SEGMENT_BYTES: the file never exceeds two bounded segments.
 */
import { spawnSync } from 'node:child_process'
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RTCPeerConnection } from '@node-webrtc-rust/sdk'

const sdkEntry = createRequire(__filename).resolve('@node-webrtc-rust/sdk')

/** Two loopback peers connect (ICE + DTLS), then close. */
const CHILD = `
const { RTCPeerConnection } = require(${JSON.stringify(sdkEntry)})
const a = new RTCPeerConnection({ iceServers: [] })
const b = new RTCPeerConnection({ iceServers: [] })
const up = (pc) => new Promise((resolve, reject) => {
  const t = setTimeout(() => reject(new Error('connect timeout ' + pc.connectionState)), 20000)
  const chk = () => { if (pc.connectionState === 'connected') { clearTimeout(t); resolve() } }
  pc.onconnectionstatechange = chk; chk()
})
;(async () => {
  const dc = a.createDataChannel('x')
  await dc.nativeReady
  const offer = await a.createOffer(); await a.setLocalDescription(offer)
  await a.gatheringComplete()
  await b.setRemoteDescription(a.localDescription.toJSON())
  const answer = await b.createAnswer(); await b.setLocalDescription(answer)
  await b.gatheringComplete()
  await a.setRemoteDescription(b.localDescription.toJSON())
  await Promise.all([up(a), up(b)])
  a.close(); b.close()
  setTimeout(() => process.exit(0), 200)
})().catch((e) => { console.error(String(e)); process.exit(1) })
`

function runChild(env: Record<string, string | undefined>): {
  status: number | null
  stderr: string
} {
  const merged: NodeJS.ProcessEnv = { ...process.env }
  delete merged.WNR_LOG_FILE
  delete merged.RUST_LOG
  delete merged.WNR_LOG_SEGMENT_BYTES
  for (const [k, v] of Object.entries(env)) if (v !== undefined) merged[k] = v
  const r = spawnSync(process.execPath, ['-e', CHILD], {
    env: merged,
    encoding: 'utf8',
    timeout: 60_000,
  })
  return { status: r.status, stderr: r.stderr }
}

describe.skipIf(typeof RTCPeerConnection !== 'function')('native trace env gating', () => {
  let dir: string
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'wnr-trace-gating-'))
  })
  afterEach(() => {
    rmSync(dir, { recursive: true, force: true })
  })

  it('creates no file when WNR_LOG_FILE is unset, even with RUST_LOG set', () => {
    const r = runChild({ RUST_LOG: 'trace' })
    expect(r.status, r.stderr).toBe(0)
    expect(readdirSync(dir)).toEqual([])
    expect(existsSync(join(dir, 'native-0.log'))).toBe(false)
  })

  it('writes one file per process with transport targets only', () => {
    const r = runChild({ WNR_LOG_FILE: join(dir, 'native-{pid}.log'), RUST_LOG: 'debug' })
    expect(r.status, r.stderr).toBe(0)
    const files = readdirSync(dir)
    expect(files).toHaveLength(1)
    expect(files[0]).toMatch(/^native-\d+\.log$/)
    const lines = readFileSync(join(dir, files[0]), 'utf8').split('\n').filter(Boolean)
    expect(lines.length).toBeGreaterThan(0)
    const targets = new Set<string>()
    for (const line of lines) {
      const m = /^\[\S+ +[A-Z]+ +([^\]]+)\]/.exec(line)
      if (m) targets.add(m[1])
    }
    expect(targets.size).toBeGreaterThan(0)
    for (const t of targets) {
      expect(t, `target ${t}`).toMatch(/^(dtls|webrtc\w*|rtp|rtcp|stun|turn|sdp)(::|$)/)
      expect(t).not.toMatch(/^node_webrtc_rust/)
    }
    expect([...targets].some((t) => t.startsWith('webrtc'))).toBe(true)
    // The loopback pair ran a real ICE + DTLS handshake, so both transports must show up.
    expect(
      [...targets].some((t) => /^(dtls|webrtc_dtls|webrtc::dtls_transport)(::|$)/.test(t)),
    ).toBe(true)
    expect([...targets].some((t) => /^webrtc_ice(::|$)/.test(t))).toBe(true)
  })

  it('keeps the file within two bounded segments', () => {
    const seg = 20_000
    const r = runChild({
      WNR_LOG_FILE: join(dir, 'native-{pid}.log'),
      RUST_LOG: 'trace',
      WNR_LOG_SEGMENT_BYTES: String(seg),
    })
    expect(r.status, r.stderr).toBe(0)
    const files = readdirSync(dir).sort()
    expect(files).toHaveLength(2)
    expect(files[1]).toMatch(/\.log\.1$/)
    for (const f of files) expect(statSync(join(dir, f)).size).toBeLessThanOrEqual(seg)
  })
})
