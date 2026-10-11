import { readdirSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

/**
 * Helpers unit tests (`*.test.ts` without `.integration`) run in the quality job against the
 * native binding installed from npm, i.e. the previous release. A unit test that builds the
 * real native mix graph therefore tests last release's Rust code and breaks every release that
 * changes the mixer (0.9.42: `mixStats is not a function`). Tests that need the native graph
 * must be `*.integration.test.ts`, which run after `build:native` from source.
 */
const NATIVE_GRAPH = /new\s+AudioMixGraph\s*\(/

describe('helpers unit tests stay independent of the native binding', () => {
  it('no unit test constructs the native AudioMixGraph', () => {
    const dir = dirname(fileURLToPath(import.meta.url))
    const offenders = readdirSync(dir)
      .filter((f) => f.endsWith('.test.ts') && !f.endsWith('.integration.test.ts'))
      .filter((f) => NATIVE_GRAPH.test(readFileSync(join(dir, f), 'utf8')))
    expect(offenders, 'move these to *.integration.test.ts').toEqual([])
  })
})
