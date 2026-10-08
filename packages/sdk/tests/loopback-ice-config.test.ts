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

const ICE_URL = /['"`](?:stuns?|turns?):[^'"`]*['"`]/
/** Rust string literal holding a STUN/TURN url. */
const RUST_ICE_URL = /"(?:stuns?|turns?):[^"]*"/

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
      ...files.filter((f) => ICE_URL.test(readFileSync(f, 'utf8'))),
      ...rustFiles.filter((f) => RUST_ICE_URL.test(readFileSync(f, 'utf8'))),
    ]
      .map(rel)
      .filter((r) => !(r in ICE_SERVER_ALLOWLIST))
    expect(offenders).toEqual([])
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
