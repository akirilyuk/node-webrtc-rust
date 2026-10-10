import { execFileSync, spawnSync } from 'node:child_process'
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

/**
 * Guard: a Sherpa roundtrip script's in-process wall-clock limit (`installRoundtripWallClockTimeout`)
 * must fit inside its CI step cap (`sherpa_roundtrip_timeout_sec` in scripts/ci/run-sherpa-example-ci.sh).
 * Otherwise the CI step is killed first (exit 124) and the real failure is hidden behind a timeout.
 */
const here = dirname(fileURLToPath(import.meta.url))
const exampleRoot = join(here, '..')
const repoRoot = join(exampleRoot, '..', '..')

/** Startup (npm + tsx + model load) and teardown outside the in-process clock. */
const STEP_OVERHEAD_MS = 20_000

/** Floors of `roundtripWallClockMs(config, profile)`; the real value is never lower. */
const PROFILE_FLOOR_MS = { short: 75_000, long: 120_000 } as const

/** Wall computed from run-time inputs (steps x budget); not statically checkable here. */
const COMPUTED_WALL_SCRIPTS = new Set([
  'start:roundtrip-load-ci',
  'start:roundtrip-concurrent-multi-client',
])

function stepCapsSeconds(env: NodeJS.ProcessEnv = process.env): Map<string, number> {
  const out = execFileSync('bash', ['scripts/ci/run-sherpa-example-ci.sh', 'step-timeouts'], {
    cwd: repoRoot,
    env,
    encoding: 'utf8',
  })
  const caps = new Map<string, number>()
  for (const line of out.trim().split('\n')) {
    const [script, secs] = line.trim().split(/\s+/)
    caps.set(script!, Number(secs))
  }
  return caps
}

/** In-process wall in ms, or null when computed at run time. */
export function staticWallMs(source: string): number | 'computed' | null {
  const call = /installRoundtripWallClockTimeout\(([^\n]*)\)\s*$/m.exec(source)
  if (!call) return null
  const arg = call[1]!
  const profile = /roundtripWallClockMs\(\s*config\s*,\s*'(short|long)'\s*\)/.exec(arg)
  if (profile) return PROFILE_FLOOR_MS[profile[1] as 'short' | 'long']
  const literals = [...arg.matchAll(/\b(\d{1,3}(?:_\d{3})+|\d+)\b/g)].map((m) =>
    Number(m[1]!.replace(/_/g, '')),
  )
  if (literals.length > 0 && !/stepCount|wallMs/.test(arg)) return Math.max(...literals)
  return 'computed'
}

describe('Sherpa roundtrip CI step caps', () => {
  const pkg = JSON.parse(readFileSync(join(exampleRoot, 'package.json'), 'utf8')) as {
    scripts: Record<string, string>
  }

  it('staticWallMs reads literals, defaults and profile floors', () => {
    expect(staticWallMs('  installRoundtripWallClockTimeout(120_000)\n')).toBe(120_000)
    expect(
      staticWallMs(
        '  installRoundtripWallClockTimeout(Number(process.env.SHERPA_ROUNDTRIP_WALL_MS) || 240_000)\n',
      ),
    ).toBe(240_000)
    expect(
      staticWallMs("  installRoundtripWallClockTimeout(roundtripWallClockMs(config, 'long'))\n"),
    ).toBe(120_000)
    expect(staticWallMs('  installRoundtripWallClockTimeout(stepCount * stepBudgetMs)\n')).toBe(
      'computed',
    )
  })

  it('every roundtrip E2E step cap exceeds the script wall-clock limit', () => {
    const caps = stepCapsSeconds()
    expect(caps.size).toBeGreaterThan(10)
    const violations: string[] = []
    for (const [script, capSec] of caps) {
      const cmd = pkg.scripts[script]
      expect(cmd, `${script} missing from package.json`).toBeDefined()
      const file = /tsx (src\/[^\s"]+\.ts)/.exec(cmd!)?.[1]
      expect(file, `${script}: no tsx entry in "${cmd}"`).toBeDefined()
      const wall = staticWallMs(readFileSync(join(exampleRoot, file!), 'utf8'))
      if (wall === 'computed') {
        expect(
          COMPUTED_WALL_SCRIPTS.has(script),
          `${script}: wall-clock is computed; add it to COMPUTED_WALL_SCRIPTS`,
        ).toBe(true)
        continue
      }
      if (wall == null) {
        violations.push(`${script}: ${file} never calls installRoundtripWallClockTimeout`)
        continue
      }
      if (capSec * 1000 < wall + STEP_OVERHEAD_MS) {
        violations.push(
          `${script}: in-process wall ${wall} ms + ${STEP_OVERHEAD_MS} ms overhead exceeds CI step cap ${capSec}s`,
        )
      }
    }
    expect(violations).toEqual([])
  })

  it('detects a cap below the wall (lid-multi at its old 180s cap)', () => {
    const caps = stepCapsSeconds({
      ...process.env,
      CI_SHERPA_LID_MULTI_ROUNDTRIP_TIMEOUT_SEC: '180',
    })
    const capSec = caps.get('start:roundtrip-counting-echo-lid-multi')!
    const wall = staticWallMs(
      readFileSync(join(exampleRoot, 'src/roundtrip-counting-echo-lid-multi.ts'), 'utf8'),
    )
    expect(capSec).toBe(180)
    expect(typeof wall === 'number' && capSec * 1000 < wall + STEP_OVERHEAD_MS).toBe(true)
  })
})

describe('run-sherpa-roundtrip-e2e.sh re-run budget', () => {
  /** Runs the wrapper against a fake `npm` that always fails; returns its calls and exit code. */
  function runWrapper(stepCapSec: string): { status: number | null; calls: string[] } {
    const dir = mkdtempSync(join(tmpdir(), 'sherpa-e2e-wrapper-'))
    const callsFile = join(dir, 'calls.log')
    const fakeNpm = join(dir, 'npm')
    writeFileSync(
      fakeNpm,
      `#!/bin/bash\necho "voice_debug=$VOICE_DEBUG wall_max=\${SHERPA_ROUNDTRIP_WALL_MAX_MS:-}" >> "${callsFile}"\nexit 1\n`,
    )
    chmodSync(fakeNpm, 0o755)
    const proc = spawnSync('bash', ['scripts/ci/run-sherpa-roundtrip-e2e.sh', 'start:fake'], {
      cwd: repoRoot,
      env: {
        PATH: `${dir}:${process.env.PATH}`,
        HOME: process.env.HOME,
        SHERPA_EXPORT_SKIP_VALIDATE: '1',
        SHERPA_ROUNDTRIP_STEP_TIMEOUT_SEC: stepCapSec,
      },
      encoding: 'utf8',
    })
    let calls: string[] = []
    try {
      calls = readFileSync(callsFile, 'utf8').trim().split('\n')
    } catch {
      calls = []
    }
    return { status: proc.status, calls }
  }

  it('re-runs with VOICE_DEBUG=1 and a wall limit that fits in the time left under the cap', () => {
    const { status, calls } = runWrapper('100')
    expect(status).toBe(1)
    expect(calls).toHaveLength(2)
    expect(calls[0]).toBe('voice_debug=0 wall_max=')
    const match = /^voice_debug=1 wall_max=(\d+)$/.exec(calls[1]!)
    expect(match).not.toBeNull()
    // 100s cap - 10s reserve - (first pass takes well under 5s) -> 85..90s.
    expect(Number(match![1])).toBeGreaterThan(85_000)
    expect(Number(match![1])).toBeLessThanOrEqual(90_000)
  })

  it('skips the re-run when too little time is left under the cap', () => {
    const { status, calls } = runWrapper('50')
    expect(status).toBe(1)
    expect(calls).toEqual(['voice_debug=0 wall_max='])
  })
})
