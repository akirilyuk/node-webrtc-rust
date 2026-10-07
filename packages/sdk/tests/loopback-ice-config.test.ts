import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, test } from 'vitest'

import { RTCPeerConnection } from '../src'
import { defaultIceConfig } from './helpers'

/**
 * Guard for the loopback-test ICE configuration.
 *
 * A STUN/TURN url with a hostname makes `gatheringComplete()` wait for DNS resolution of that
 * host (webrtc-ice resolves it with an unbounded `lookup_host`). On a CI runner with a stalled
 * resolver, two such peers held `e2e.test.ts` past its 60 s timeout (CI run 37692990611).
 * Loopback tests must not depend on external DNS or STUN reachability.
 */

const TESTS_DIR = __dirname

/** Files that legitimately configure ICE servers, with the reason. */
const ICE_SERVER_ALLOWLIST: Record<string, string> = {
  'turn.test.ts': 'exercises a TURN relay and skips unless TURN_AVAILABLE=1',
  'sdk.test.ts': 'asserts getConfiguration() echoes the config; never gathers candidates',
  'loopback-ice-config.test.ts': 'this guard',
}

const ICE_URL = /['"`](?:stuns?|turns?):[^'"`]*['"`]/

describe('loopback ICE configuration', () => {
  test('defaultIceConfig has no ICE servers', () => {
    expect(defaultIceConfig.iceServers).toEqual([])
  })

  test('no SDK test file outside the allowlist configures a STUN/TURN url', () => {
    const offenders = readdirSync(TESTS_DIR)
      .filter((name) => name.endsWith('.ts') && !(name in ICE_SERVER_ALLOWLIST))
      .filter((name) => ICE_URL.test(readFileSync(join(TESTS_DIR, name), 'utf8')))
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
