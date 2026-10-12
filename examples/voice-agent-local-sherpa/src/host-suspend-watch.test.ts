import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

import { HostSuspendWatch } from './host-suspend-watch.js'

function fakeWatch(): { watch: HostSuspendWatch; advance: (ms: number) => void } {
  let t = 1_700_000_000_000
  const watch = new HostSuspendWatch({ now: () => t })
  return { watch, advance: (ms) => void (t += ms) }
}

describe('HostSuspendWatch', () => {
  it('reports nothing for regular ticks', () => {
    const { watch, advance } = fakeWatch()
    for (let i = 0; i < 100; i++) {
      advance(250)
      watch.tick()
    }
    expect(watch.summary()).toBeNull()
  })

  it('names a 255 s jump as a system sleep', () => {
    const { watch, advance } = fakeWatch()
    advance(255_000 + 250)
    watch.tick()
    const s = watch.summary()
    expect(s).toContain('255.0 s')
    expect(s).toContain('system sleep')
    expect(s).toContain('1 pause(s)')
  })

  it('counts and sums two jumps', () => {
    const { watch, advance } = fakeWatch()
    advance(10_250)
    watch.tick()
    advance(250)
    watch.tick()
    advance(20_250)
    watch.tick()
    const s = watch.summary()
    expect(s).toContain('30.0 s')
    expect(s).toContain('2 pause(s)')
    expect(s).toContain('longest 20.0 s')
  })

  it('ignores gaps at or below the threshold', () => {
    const { watch, advance } = fakeWatch()
    advance(1500)
    watch.tick()
    expect(watch.summary()).toBeNull()
  })
})

describe('roundtrip entry scripts', () => {
  it('every roundtrip entry script installs the wall-clock timeout (which starts the suspend watch)', () => {
    const dir = dirname(fileURLToPath(import.meta.url))
    const pkg = JSON.parse(readFileSync(join(dir, '..', 'package.json'), 'utf8')) as {
      scripts: Record<string, string>
    }
    const entryScripts = Object.entries(pkg.scripts)
      .filter(([name]) => name.startsWith('start:roundtrip'))
      .map(([, cmd]) => /tsx src\/([^\s"]+\.ts)/.exec(cmd)?.[1])
      .filter((f): f is string => f != null)
    expect(entryScripts.length).toBeGreaterThan(5)
    const missing = entryScripts.filter(
      (f) => !readFileSync(join(dir, f), 'utf8').includes('installRoundtripWallClockTimeout('),
    )
    expect(missing).toEqual([])
  })
})
