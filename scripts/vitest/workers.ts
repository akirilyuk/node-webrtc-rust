// Host-wide cap on vitest workers. Vitest has no env var for the worker count,
// so every config calls localWorkerLimit(). Locally, all vitest runs of the
// user share a budget of slot directories under /tmp (default 6 workers in
// total); a run that finds none free waits. In CI (CI set) nothing changes.
//
// Env: VITEST_HOST_WORKERS (budget, default 6), VITEST_MAX_WORKERS (workers
// wanted by one run, default 6, capped by the budget). `--maxWorkers` on the
// vitest command line still overrides the config.
//
// Keep this file free of TypeScript-only runtime syntax (enums, parameter
// properties): the cross-process test loads it through tsx.
import { mkdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { userInfo } from 'node:os'
import { join } from 'node:path'

// Fixed /tmp path, not os.tmpdir(): sessions can have different TMPDIR values,
// which would split the gate. Per uid so users do not share (or block) slots.
export const DEFAULT_SLOT_DIR = `/tmp/vitest-worker-slots-${userInfo().uid}`
const DEFAULT_BUDGET = 6
const OWNERLESS_STALE_MS = 10_000
const DEFAULT_POLL_MS = 1000
const DEFAULT_TIMEOUT_MS = 60 * 60 * 1000

export type SlotGrant = { count: number; release: () => void }

export type AcquireOptions = {
  dir: string
  budget: number
  want: number
  pollMs?: number
  timeoutMs?: number
  log?: (message: string) => void
}

function readOwner(slot: string): number | undefined {
  try {
    const pid = Number.parseInt(readFileSync(join(slot, 'owner'), 'utf8'), 10)
    return Number.isInteger(pid) && pid > 0 ? pid : undefined
  } catch {
    return undefined
  }
}

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch (err) {
    return (err as NodeJS.ErrnoException).code !== 'ESRCH'
  }
}

// True when the slot's owner is gone (killed, OOM) or never wrote its pid.
function isStale(slot: string): { stale: boolean; owner?: number } {
  const owner = readOwner(slot)
  if (owner !== undefined) return { stale: !isAlive(owner), owner }
  try {
    return { stale: Date.now() - statSync(slot).mtimeMs > OWNERLESS_STALE_MS }
  } catch {
    return { stale: false }
  }
}

let tmpCounter = 0

// Reclaim a stale slot. A per-slot lock dir keeps two reclaimers apart: the
// holder re-checks staleness under the lock, then renames the slot away and
// deletes it. A lock left by a killed reclaimer is removed once it is old.
function reclaim(slot: string): void {
  const lock = `${slot}.lock`
  try {
    mkdirSync(lock)
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code !== 'EEXIST') throw err
    try {
      if (Date.now() - statSync(lock).mtimeMs > OWNERLESS_STALE_MS) {
        rmSync(lock, { recursive: true, force: true })
      }
    } catch {
      // lock vanished: its holder finished
    }
    return
  }
  try {
    if (isStale(slot).stale) {
      const tomb = `${slot}.stale-${process.pid}-${tmpCounter++}`
      try {
        renameSync(slot, tomb)
        rmSync(tomb, { recursive: true, force: true })
      } catch (err) {
        if ((err as NodeJS.ErrnoException).code !== 'ENOENT') throw err
      }
    }
  } finally {
    rmSync(lock, { recursive: true, force: true })
  }
}

// Publish a slot atomically: the owner file is written into a private dir that
// is renamed to the slot name, so a slot never exists without its owner.
function tryTake(slot: string): boolean {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const tmp = `${slot}.tmp-${process.pid}-${tmpCounter++}`
    mkdirSync(tmp)
    try {
      writeFileSync(join(tmp, 'owner'), String(process.pid))
      renameSync(tmp, slot)
      return true
    } catch (err) {
      rmSync(tmp, { recursive: true, force: true })
      const code = (err as NodeJS.ErrnoException).code
      if (code !== 'EEXIST' && code !== 'ENOTEMPTY') throw err
    }
    if (!isStale(slot).stale) return false
    reclaim(slot)
  }
  return false
}

function holders(dir: string, budget: number): string {
  const pids: number[] = []
  for (let i = 0; i < budget; i += 1) {
    const pid = readOwner(join(dir, `slot-${i}`))
    if (pid !== undefined) pids.push(pid)
  }
  return pids.join(', ') || 'unknown'
}

export async function acquireSlots(opts: AcquireOptions): Promise<SlotGrant> {
  const { dir, budget, want } = opts
  const pollMs = opts.pollMs ?? DEFAULT_POLL_MS
  const timeoutMs = opts.timeoutMs ?? DEFAULT_TIMEOUT_MS
  const log = opts.log ?? ((m: string) => process.stderr.write(`${m}\n`))
  mkdirSync(dir, { recursive: true, mode: 0o700 })

  const startedAt = Date.now()
  let warned = false
  for (;;) {
    const held: string[] = []
    for (let i = 0; i < budget && held.length < want; i += 1) {
      const slot = join(dir, `slot-${i}`)
      if (tryTake(slot)) held.push(slot)
    }
    if (held.length > 0) {
      let released = false
      return {
        count: held.length,
        release: () => {
          if (released) return
          released = true
          for (const slot of held) {
            // Only remove a slot that still names us as owner.
            if (readOwner(slot) !== process.pid) continue
            const tomb = `${slot}.stale-${process.pid}-${tmpCounter++}`
            try {
              renameSync(slot, tomb)
              rmSync(tomb, { recursive: true, force: true })
            } catch {
              // already gone
            }
          }
        },
      }
    }
    if (!warned) {
      warned = true
      log(`[vitest] waiting for a worker slot (${budget} in use by pids ${holders(dir, budget)})`)
    }
    if (Date.now() - startedAt >= timeoutMs) {
      throw new Error(
        `[vitest] no worker slot after ${timeoutMs} ms; held by pids ${holders(dir, budget)}`,
      )
    }
    await new Promise((resolve) => setTimeout(resolve, pollMs))
  }
}

function positiveInt(raw: string | undefined, fallback: number): number {
  if (raw === undefined || !/^\d+$/.test(raw.trim())) return fallback
  const n = Number.parseInt(raw, 10)
  return n >= 1 ? n : fallback
}

export type LocalLimitOptions = {
  env?: Record<string, string | undefined>
  dir?: string
  pollMs?: number
  timeoutMs?: number
  log?: (message: string) => void
}

const GRANT_KEY = Symbol.for('vitest.workers.grant')
type GrantEntry = { pid: number; grant: Promise<SlotGrant> }
type WithGrant = { [GRANT_KEY]?: GrantEntry }

// Vite bundles this file into every config load, so each (re)load gets a fresh
// module copy. The grant therefore lives on globalThis, not in module state.
// Test hook: forget the per-process grant (does not release it).
export function resetLocalGrantForTests(): void {
  delete (globalThis as WithGrant)[GRANT_KEY]
}

export async function localWorkerLimit(
  opts: LocalLimitOptions = {},
): Promise<{ maxWorkers?: number }> {
  const env = opts.env ?? process.env
  if (env.CI) return {}
  const budget = positiveInt(env.VITEST_HOST_WORKERS, DEFAULT_BUDGET)
  const want = Math.min(positiveInt(env.VITEST_MAX_WORKERS, DEFAULT_BUDGET), budget)
  const g = globalThis as WithGrant
  let entry = g[GRANT_KEY]
  if (!entry || entry.pid !== process.pid) {
    const grant = acquireSlots({
      dir: opts.dir ?? DEFAULT_SLOT_DIR,
      budget,
      want,
      pollMs: opts.pollMs,
      timeoutMs: opts.timeoutMs,
      log: opts.log,
    })
    entry = { pid: process.pid, grant }
    g[GRANT_KEY] = entry
    // Handlers are installed only here, when the entry is first created.
    grant.then(installReleaseHandlers, () => {
      if (g[GRANT_KEY] === entry) delete g[GRANT_KEY]
    })
  }
  try {
    const { count } = await entry.grant
    return { maxWorkers: count }
  } catch (err) {
    // A filesystem error (EACCES, EROFS, ...) skips the host gate only; the
    // cap still applies. The timeout error has no code and is rethrown.
    const code = (err as NodeJS.ErrnoException).code
    if (typeof code !== 'string') throw err
    const log = opts.log ?? ((m: string) => process.stderr.write(`${m}\n`))
    log(
      `[vitest] worker slot gate unavailable (${code}); using maxWorkers=${want} without the host gate`,
    )
    return { maxWorkers: want }
  }
}

function installReleaseHandlers(grant: SlotGrant): void {
  process.once('exit', grant.release)
  for (const sig of ['SIGINT', 'SIGTERM', 'SIGHUP'] as const) {
    process.once(sig, () => {
      grant.release()
      // Our once-listener removed the default; re-raise only if nobody else
      // (e.g. vitest's own graceful shutdown) is listening.
      if (process.listenerCount(sig) === 0) process.kill(process.pid, sig)
    })
  }
}
