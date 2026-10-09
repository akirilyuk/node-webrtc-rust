import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative, sep } from 'node:path'

import { describe, expect, test } from 'vitest'

import { RTCPeerConnection } from '../src'
import { defaultIceConfig as helpersDefaultIceConfig } from '../../helpers/tests/mix-three-client-helpers'
import { defaultIceConfig } from './helpers'

/**
 * Guard for the loopback-test ICE configuration, across every package.
 *
 * A STUN/TURN url with a hostname makes `gatheringComplete()` wait for DNS resolution of that
 * host (webrtc-ice resolves it with an unbounded `lookup_host`). On a CI runner with a stalled
 * resolver, two such peers held `e2e.test.ts` past its 60 s timeout (CI run 37692990611).
 * Loopback tests must not depend on external DNS or STUN reachability.
 *
 * 2nd occurrence: helpers SessionPod integration tests, main run 37801440974 (the first fix
 * only covered `packages/sdk/tests`), so the scan now covers every `packages/*\/tests`.
 *
 * 3rd occurrence: main run 37816046524, `examples/voice-agent/src/shared-loopback.ts` (two peers
 * in one process on `stun:stun.l.google.com`; CI step `sherpa e2e
 * start:roundtrip-concurrent-multi-client` timed out waiting for agent ontrack). The guard now
 * also covers `examples/**\/*.ts`.
 */

const REPO_ROOT = join(__dirname, '..', '..', '..')
const PACKAGES_DIR = join(REPO_ROOT, 'packages')

/** Repo-relative (posix) paths that legitimately configure ICE servers, with the reason. */
const ICE_SERVER_ALLOWLIST: Record<string, string> = {
  'packages/sdk/tests/turn.test.ts': 'exercises a TURN relay and skips unless TURN_AVAILABLE=1',
  'packages/sdk/tests/sdk.test.ts':
    'asserts getConfiguration() echoes the config; never gathers candidates',
  'packages/sdk/tests/loopback-ice-config.test.ts': 'this guard',
}

const EXAMPLES_DIR = join(REPO_ROOT, 'examples')

/**
 * Repo-relative (posix) example sources that keep a STUN/TURN url, with the reason. None of
 * these is executed by CI with both peers local (CI runs the sherpa roundtrips, which use
 * `examples/voice-agent/src/shared-loopback.ts` and `DEMO_ICE_SERVERS`, plus vitest).
 */
const EXAMPLES_ICE_SERVER_ALLOWLIST: Record<string, string> = {
  'examples/audio-cosine/src/index.ts': 'node demo, not run by CI',
  'examples/browser-cosine-chat/src/index.ts': 'browser demo, not run by CI',
  'examples/conference-room-manual-signaling/src/index.ts': 'browser demo, not run by CI',
  'examples/conference-room/src/index.ts': 'browser demo, not run by CI',
  'examples/peer-connection/src/parity-features.ts': 'setConfiguration demo, not run by CI',
  'examples/voice-agent-browser/src/index.ts': 'browser demo, not run by CI',
  'examples/voice-agent-local-sherpa-multi-client/src/index.ts': 'browser demo, not run by CI',
  'examples/voice-agent-local-sherpa-multi-client/src/mix-groups.ts': 'browser demo, not run by CI',
  'examples/voice-agent-local-sherpa/src/index.ts': 'browser demo, not run by CI',
  'examples/voice-agent-multi-session-pod/src/index.ts': 'browser demo, not run by CI',
}

const ICE_URL = /['"`](?:stuns?|turns?):[^'"`]*['"`]/
/** Rust string literal holding a STUN/TURN url. */
const RUST_ICE_URL = /"(?:stuns?|turns?):[^"]*"/

const ICE_URL_ALL = new RegExp(ICE_URL.source, 'g')
const RUST_ICE_URL_ALL = new RegExp(RUST_ICE_URL.source, 'g')

/** `.invalid` is reserved (RFC 2606) and never resolves, so such urls cannot reintroduce DNS or STUN dependence. */
function isReservedInvalidUrl(literal: string): boolean {
  return /\.invalid(?::\d+)?(?:\?[^'"`]*)?['"`]$/.test(literal)
}

/** True when the source holds a STUN/TURN url literal whose host is not `.invalid`. */
function hasRealIceUrl(source: string, re: RegExp): boolean {
  return (source.match(re) ?? []).some((m) => !isReservedInvalidUrl(m))
}

function listTestFiles(dir: string, ext = '.ts'): string[] {
  const out: string[] = []
  for (const name of readdirSync(dir)) {
    if (name === 'node_modules' || name === 'dist') continue
    const full = join(dir, name)
    if (statSync(full).isDirectory()) out.push(...listTestFiles(full, ext))
    else if (name.endsWith(ext)) out.push(full)
  }
  return out
}

function allExampleSourceFiles(): string[] {
  const out: string[] = []
  const walk = (dir: string) => {
    for (const name of readdirSync(dir)) {
      if (['node_modules', 'dist', '.models', 'generated'].includes(name)) continue
      const full = join(dir, name)
      if (statSync(full).isDirectory()) walk(full)
      else if (name.endsWith('.ts') && !name.endsWith('.d.ts') && !name.includes('.generated.'))
        out.push(full)
    }
  }
  walk(EXAMPLES_DIR)
  return out
}

function allPackageTestFiles(): string[] {
  return readdirSync(PACKAGES_DIR)
    .map((pkg) => join(PACKAGES_DIR, pkg, 'tests'))
    .filter((dir) => {
      try {
        return statSync(dir).isDirectory()
      } catch {
        return false
      }
    })
    .flatMap((dir) => listTestFiles(dir))
}

function allCrateTestFiles(): string[] {
  const crates = join(REPO_ROOT, 'crates')
  return readdirSync(crates)
    .map((c) => join(crates, c, 'tests'))
    .filter((dir) => {
      try {
        return statSync(dir).isDirectory()
      } catch {
        return false
      }
    })
    .flatMap((dir) => listTestFiles(dir, '.rs'))
}

describe('loopback ICE configuration', () => {
  test('defaultIceConfig has no ICE servers', () => {
    expect(defaultIceConfig.iceServers).toEqual([])
  })

  test("every package's defaultIceConfig has no ICE servers", () => {
    expect(defaultIceConfig.iceServers).toEqual([])
    expect(helpersDefaultIceConfig.iceServers).toEqual([])
  })

  test('no test file in packages/*/tests or crates/*/tests outside the allowlist configures a STUN/TURN url', () => {
    const files = allPackageTestFiles()
    expect(files.length).toBeGreaterThan(10)
    const rustFiles = allCrateTestFiles()
    expect(rustFiles.length).toBeGreaterThan(0)
    const rel = (f: string) => relative(REPO_ROOT, f).split(sep).join('/')
    const offenders = [
      ...files.filter((f) => hasRealIceUrl(readFileSync(f, 'utf8'), ICE_URL_ALL)),
      ...rustFiles.filter((f) => hasRealIceUrl(readFileSync(f, 'utf8'), RUST_ICE_URL_ALL)),
    ]
      .map(rel)
      .filter((r) => !(r in ICE_SERVER_ALLOWLIST))
    expect(offenders).toEqual([])
  })

  test('no example source outside the allowlist configures a STUN/TURN url', () => {
    const files = allExampleSourceFiles()
    expect(files.length).toBeGreaterThan(10)
    const rel = (f: string) => relative(REPO_ROOT, f).split(sep).join('/')
    const offenders = files
      .filter((f) => hasRealIceUrl(readFileSync(f, 'utf8'), ICE_URL_ALL))
      .map(rel)
      .filter((r) => !(r in EXAMPLES_ICE_SERVER_ALLOWLIST))
    expect(offenders).toEqual([])
  })

  test('every examples allowlist entry still exists and still has an ICE url', () => {
    const stale = Object.keys(EXAMPLES_ICE_SERVER_ALLOWLIST).filter((r) => {
      try {
        return !hasRealIceUrl(readFileSync(join(REPO_ROOT, r), 'utf8'), ICE_URL_ALL)
      } catch {
        return true
      }
    })
    expect(stale).toEqual([])
  })

  test('guard accepts reserved .invalid hosts and flags real ones', () => {
    expect(hasRealIceUrl('"stun:does-not-exist.invalid:19302"', RUST_ICE_URL_ALL)).toBe(false)
    expect(hasRealIceUrl('"stun:stun.l.google.com:19302"', RUST_ICE_URL_ALL)).toBe(true)
    expect(
      hasRealIceUrl(
        '"stun:does-not-exist.invalid:1" "stun:stun.l.google.com:19302"',
        RUST_ICE_URL_ALL,
      ),
    ).toBe(true)
  })

  test('a peer built from defaultIceConfig gathers host candidates without ICE servers', async () => {
    const pc = new RTCPeerConnection(defaultIceConfig)
    pc.createDataChannel('probe')
    await pc.setLocalDescription(await pc.createOffer())
    await pc.gatheringComplete()
    expect(pc.localDescription?.sdp).toContain('a=candidate')
    pc.close()
  })
})
