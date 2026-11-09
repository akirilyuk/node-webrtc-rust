import { type ChildProcess, spawn } from 'node:child_process'
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  chmodSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  acquireSlots,
  localWorkerLimit,
  resetLocalGrantForTests,
  type SlotGrant,
} from '../../../scripts/vitest/workers'
import { workerCap } from '../../../scripts/vitest/cap'

const HELPER_URL = pathToFileURL(join(__dirname, '../../../scripts/vitest/workers.ts')).href
const REPO_ROOT = join(__dirname, '../../..')
const quiet = () => {}

let dir: string
let children: ChildProcess[] = []
let grants: SlotGrant[] = []

beforeEach(() => {
  dir = join(mkdtempSync(join(tmpdir(), 'vitest-slots-')), 'slots')
  resetLocalGrantForTests()
})

afterEach(() => {
  for (const g of grants) g.release()
  grants = []
  for (const c of children) c.kill('SIGKILL')
  children = []
  rmSync(join(dir, '..'), { recursive: true, force: true })
})

function liveChild(): Promise<ChildProcess> {
  const child = spawn(process.execPath, ['-e', "console.log('up');setInterval(()=>{},1e3)"], {
    stdio: ['ignore', 'pipe', 'ignore'],
  })
  children.push(child)
  return new Promise((resolve) => child.stdout!.once('data', () => resolve(child)))
}

function claimSlots(owner: number, indexes: number[]) {
  for (const i of indexes) {
    const slot = join(dir, `slot-${i}`)
    mkdirSync(slot, { recursive: true })
    writeFileSync(join(slot, 'owner'), String(owner))
  }
}

function slotNames(): string[] {
  return existsSync(dir) ? readdirSync(dir).filter((n) => /^slot-\d+$/.test(n)) : []
}

describe('localWorkerLimit', () => {
  it('returns {} and creates no slot dir when CI is set', async () => {
    expect(await localWorkerLimit({ env: { CI: '1' }, dir })).toEqual({})
    expect(existsSync(dir)).toBe(false)
  })

  it('takes the configured want and a second call reuses the same grant', async () => {
    const env = { VITEST_HOST_WORKERS: '6', VITEST_MAX_WORKERS: '4' }
    expect(await localWorkerLimit({ env, dir, log: quiet })).toEqual({ maxWorkers: 4 })
    expect(slotNames()).toHaveLength(4)
    expect(await localWorkerLimit({ env, dir, log: quiet })).toEqual({ maxWorkers: 4 })
    expect(slotNames()).toHaveLength(4)
  })

  it('falls back to the default 6 on invalid values and caps want by the budget', async () => {
    for (const bad of ['0', 'abc']) {
      resetLocalGrantForTests()
      const r = await localWorkerLimit({
        env: { VITEST_HOST_WORKERS: bad },
        dir,
        log: quiet,
      })
      expect(r).toEqual({ maxWorkers: 6 })
      // release by removing our slots; the next iteration takes them again
      rmSync(dir, { recursive: true, force: true })
    }
    resetLocalGrantForTests()
    const capped = await localWorkerLimit({
      env: { VITEST_HOST_WORKERS: '6', VITEST_MAX_WORKERS: '10' },
      dir,
      log: quiet,
    })
    expect(capped).toEqual({ maxWorkers: 6 })
  })
})

describe('workerCap', () => {
  it('returns {} in CI and maxWorkers with a lowered minWorkers locally', async () => {
    expect(await workerCap({ env: { CI: '1' }, dir })).toEqual({})
    expect(await workerCap({ env: { VITEST_MAX_WORKERS: '3' }, dir, log: quiet })).toEqual({
      maxWorkers: 3,
      minWorkers: 1,
    })
  })
})

describe('localWorkerLimit grant sharing and fallback', () => {
  it('a re-imported helper reuses the grant', async () => {
    const opts = { env: {}, dir, timeoutMs: 50, pollMs: 5, log: quiet }
    expect(await localWorkerLimit(opts)).toEqual({ maxWorkers: 6 })
    vi.resetModules()
    const fresh = await import('../../../scripts/vitest/workers')
    expect(await fresh.localWorkerLimit(opts)).toEqual({ maxWorkers: 6 })
    expect(slotNames()).toHaveLength(6)
  })

  it('skips the host gate but keeps the cap when the slot dir is unusable', async () => {
    const parent = mkdtempSync(join(tmpdir(), 'vitest-ro-'))
    chmodSync(parent, 0o500)
    const warnings: string[] = []
    try {
      const result = await localWorkerLimit({
        env: { VITEST_MAX_WORKERS: '3' },
        dir: join(parent, 'sub', 'slots'),
        log: (m) => warnings.push(m),
      })
      expect(result).toEqual({ maxWorkers: 3 })
      expect(warnings).toHaveLength(1)
      expect(warnings[0]).toMatch(/EACCES|EROFS|EPERM/)
    } finally {
      chmodSync(parent, 0o700)
      rmSync(parent, { recursive: true, force: true })
    }
  })
})

describe('acquireSlots', () => {
  const base = () => ({ dir, budget: 6, want: 6, pollMs: 5, log: quiet })

  it('takes 6 slots from an empty dir and writes its pid as owner', async () => {
    const g = await acquireSlots(base())
    grants.push(g)
    expect(g.count).toBe(6)
    for (let i = 0; i < 6; i += 1) {
      expect(readFileSync(join(dir, `slot-${i}`, 'owner'), 'utf8')).toBe(String(process.pid))
    }
  })

  it('grants only the free slots when 4 are held by a live process', async () => {
    const child = await liveChild()
    claimSlots(child.pid!, [0, 1, 2, 3])
    const g = await acquireSlots(base())
    grants.push(g)
    expect(g.count).toBe(2)
  })

  it('stays pending while all 6 are held by a live process, then resolves with 6 once it dies', async () => {
    const child = await liveChild()
    claimSlots(child.pid!, [0, 1, 2, 3, 4, 5])
    let polls = 0
    const warnings: string[] = []
    const pending = acquireSlots({
      ...base(),
      log: (m) => {
        warnings.push(m)
      },
    })
    let settled = false
    void pending.then(() => {
      settled = true
    })
    // Hold the exclusion window: let several poll intervals pass while rejected.
    while (polls < 3) {
      await new Promise((r) => setTimeout(r, 10))
      polls += 1
    }
    expect(settled).toBe(false)
    expect(warnings).toHaveLength(1)
    expect(warnings[0]).toContain(`pids ${child.pid}`)
    child.kill('SIGKILL')
    const g = await pending
    grants.push(g)
    expect(g.count).toBe(6)
  })

  it('reclaims a slot whose owner pid is dead', async () => {
    const child = await liveChild()
    const deadPid = child.pid!
    await new Promise((r) => {
      child.once('exit', r)
      child.kill('SIGKILL')
    })
    claimSlots(deadPid, [0, 1, 2, 3, 4, 5])
    const g = await acquireSlots(base())
    grants.push(g)
    expect(g.count).toBe(6)
    expect(readFileSync(join(dir, 'slot-0', 'owner'), 'utf8')).toBe(String(process.pid))
  })

  it('leaves a slot under a fresh reclaim lock alone', async () => {
    const child = await liveChild()
    const deadPid = child.pid!
    await new Promise((r) => {
      child.once('exit', r)
      child.kill('SIGKILL')
    })
    claimSlots(deadPid, [0, 1, 2, 3, 4, 5])
    mkdirSync(join(dir, 'slot-0.lock'))
    const g = await acquireSlots(base())
    grants.push(g)
    expect(g.count).toBe(5)
    expect(readFileSync(join(dir, 'slot-0', 'owner'), 'utf8')).toBe(String(deadPid))
  })

  it('removes its slot dirs on release', async () => {
    const g = await acquireSlots(base())
    expect(slotNames()).toHaveLength(6)
    g.release()
    g.release()
    expect(slotNames()).toHaveLength(0)
  })

  it('times out with an error naming the holders', async () => {
    const child = await liveChild()
    claimSlots(child.pid!, [0, 1, 2, 3, 4, 5])
    await expect(acquireSlots({ ...base(), timeoutMs: 30 })).rejects.toThrow(
      `held by pids ${child.pid}`,
    )
  })
})

describe('across processes', () => {
  const script = `
    const { acquireSlots } = await import(${JSON.stringify(HELPER_URL)});
    let g;
    try {
      g = await acquireSlots({
        dir: process.env.SLOT_DIR, budget: 6, want: 6, pollMs: 5,
        log: (m) => console.log("WAITING"),
      });
    } catch (e) { console.log("ERR " + e.message); process.exit(1); }
    console.log("GOT " + g.count);
    process.stdin.on("end", () => { g.release(); process.exit(0); });
    process.stdin.resume();
  `

  function runner(source: string = script): {
    child: ChildProcess
    line: (prefix: string) => Promise<string>
  } {
    const child = spawn(
      process.execPath,
      ['--import', 'tsx', '--input-type=module', '-e', source],
      {
        cwd: REPO_ROOT,
        env: { ...process.env, SLOT_DIR: dir },
        stdio: ['pipe', 'pipe', 'inherit'],
      },
    )
    children.push(child)
    const lines: string[] = []
    const waiters: Array<() => void> = []
    let buf = ''
    child.stdout!.on('data', (d: Buffer) => {
      buf += d.toString()
      let nl: number
      while ((nl = buf.indexOf('\n')) >= 0) {
        lines.push(buf.slice(0, nl))
        buf = buf.slice(nl + 1)
      }
      waiters.splice(0).forEach((w) => w())
    })
    const line = async (prefix: string) => {
      for (;;) {
        const hit = lines.find((l) => l.startsWith(prefix))
        if (hit) return hit
        await new Promise<void>((r) => waiters.push(r))
      }
    }
    return { child, line }
  }

  it('a second process waits until the first exits', async () => {
    const first = runner()
    expect(await first.line('GOT')).toBe('GOT 6')
    const second = runner()
    await second.line('WAITING')
    first.child.stdin!.end()
    expect(await second.line('GOT')).toBe('GOT 6')
  })
  // One pass, no waiting: GOT n, NONE (no slot free) or ERR msg (a real failure).
  const onePassScript = `
    const { acquireSlots } = await import(${JSON.stringify(HELPER_URL)});
    let g;
    try {
      g = await acquireSlots({
        dir: process.env.SLOT_DIR, budget: 6, want: 6, pollMs: 5, timeoutMs: 0,
        log: () => {},
      });
    } catch (e) {
      console.log(e.code ? "ERR " + e.message : "NONE");
      process.exit(0);
    }
    console.log("GOT " + g.count);
    process.stdin.on("end", () => { g.release(); process.exit(0); });
    process.stdin.resume();
  `

  it('concurrent reclaimers never over-grant', async () => {
    const owned = (pid: number) =>
      slotNames().filter((s) => readFileSync(join(dir, s, 'owner'), 'utf8') === String(pid)).length
    for (let round = 0; round < 10; round += 1) {
      rmSync(dir, { recursive: true, force: true })
      const dead = await liveChild()
      const deadPid = dead.pid!
      await new Promise((r) => {
        dead.once('exit', r)
        dead.kill('SIGKILL')
      })
      claimSlots(deadPid, [0, 1, 2, 3, 4, 5])
      const runners = [0, 1, 2, 3].map(() => runner(onePassScript))
      const outcomes = await Promise.all(
        runners.map((r) => Promise.race([r.line('GOT'), r.line('NONE'), r.line('ERR')])),
      )
      expect(outcomes.filter((o) => o.startsWith('ERR'))).toEqual([])
      const got = outcomes
        .filter((o) => o.startsWith('GOT'))
        .reduce((sum, o) => sum + Number(o.split(' ')[1]), 0)
      expect(got).toBeLessThanOrEqual(6)
      // On-disk ownership must match each runner's claim exactly.
      for (const [i, o] of outcomes.entries()) {
        if (o.startsWith('GOT')) {
          expect(owned(runners[i].child.pid!)).toBe(Number(o.split(' ')[1]))
        }
      }
      for (const r of runners) {
        r.child.stdin!.end()
        r.child.kill('SIGKILL')
      }
    }
  }, 60_000)
})
