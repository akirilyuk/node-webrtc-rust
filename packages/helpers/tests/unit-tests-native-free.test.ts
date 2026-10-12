import { readdirSync, readFileSync } from 'node:fs'
import { basename, dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

/**
 * Helpers unit tests (`*.test.ts` without `.integration`) run in the quality job against the
 * native binding installed from npm, i.e. the previous release. A unit test that builds the
 * real native mix graph therefore tests last release's Rust code and breaks every release that
 * changes the mixer (0.9.42: `mixStats is not a function`). The same holds for a real
 * `RTCPeerConnection` loopback. Tests that need the native binding (mix graph, peer
 * connections) must be `*.integration.test.ts`, which run after `build:native` from source.
 */
const NATIVE_GRAPH = /new\s+AudioMixGraph\s*\(/
/** A value import of RTCPeerConnection loads the native binding; `import type` does not. */
const NATIVE_PEER_IMPORT =
  /import\s+(?!type\b)\{[^}]*(?<!\btype\s+)\bRTCPeerConnection\b[^}]*\}\s*from\s*['"]@node-webrtc-rust\/(?:sdk|bindings)['"]/

function nativeOffenses(source: string): string[] {
  const out: string[] = []
  if (NATIVE_GRAPH.test(source)) out.push('AudioMixGraph')
  if (NATIVE_PEER_IMPORT.test(source)) out.push('RTCPeerConnection import')
  return out
}

describe('helpers unit tests stay independent of the native binding', () => {
  it('no unit test uses the native AudioMixGraph or RTCPeerConnection', () => {
    const dir = dirname(fileURLToPath(import.meta.url))
    const offenders = readdirSync(dir)
      .filter((f) => f.endsWith('.test.ts') && !f.endsWith('.integration.test.ts'))
      // this file holds the probe strings below
      .filter((f) => f !== basename(fileURLToPath(import.meta.url)))
      .filter((f) => nativeOffenses(readFileSync(join(dir, f), 'utf8')).length > 0)
    expect(offenders, 'move these to *.integration.test.ts').toEqual([])
  })

  it.each([
    ["import { RTCPeerConnection } from '@node-webrtc-rust/sdk'", ['RTCPeerConnection import']],
    [
      'import { RTCSessionDescription, RTCPeerConnection } from "@node-webrtc-rust/bindings"',
      ['RTCPeerConnection import'],
    ],
    [
      "import {\n  RTCPeerConnection,\n} from '@node-webrtc-rust/sdk'",
      ['RTCPeerConnection import'],
    ],
    ['const g = new AudioMixGraph(opts)', ['AudioMixGraph']],
  ])('flags %j', (source, expected) => {
    expect(nativeOffenses(source)).toEqual(expected)
  })

  it.each([
    "import type { RTCPeerConnection } from '@node-webrtc-rust/sdk'",
    "import { type RTCPeerConnection } from '@node-webrtc-rust/sdk'",
    "import type { RTCIceServer, RTCPeerConnection } from '@node-webrtc-rust/sdk'",
    "import { RTCPeerConnection } from './fake-peer.js'",
    "import { RTCSessionDescription } from '@node-webrtc-rust/sdk'",
  ])('allows %j', (source) => {
    expect(nativeOffenses(source)).toEqual([])
  })
})
