/**
 * Detects that the host process did not run for a while (laptop sleep, SIGSTOP, blocked event loop).
 * Wall-clock timeouts that fire right after such a pause say nothing about the code under test.
 */

export type HostSuspendGap = { atMs: number; gapMs: number }

export type HostSuspendWatchOptions = {
  now?: () => number
  /** A tick later than this since the previous one counts as a pause. */
  gapThresholdMs?: number
}

const MAX_GAPS = 20

export class HostSuspendWatch {
  private readonly now: () => number
  private readonly gapThresholdMs: number
  private lastTick: number
  private timer: ReturnType<typeof setInterval> | null = null
  private intervalMs = 250
  private gapList: HostSuspendGap[] = []
  private gapCount = 0
  private totalGapMs = 0

  constructor(options: HostSuspendWatchOptions = {}) {
    this.now = options.now ?? Date.now
    this.gapThresholdMs = options.gapThresholdMs ?? 2000
    this.lastTick = this.now()
  }

  start(intervalMs = 250): void {
    if (this.timer) return
    this.intervalMs = intervalMs
    this.lastTick = this.now()
    this.timer = setInterval(() => this.tick(), intervalMs)
    this.timer.unref()
  }

  stop(): void {
    if (this.timer) clearInterval(this.timer)
    this.timer = null
  }

  /** Public for tests. */
  tick(): void {
    const now = this.now()
    const elapsed = now - this.lastTick
    if (elapsed > this.gapThresholdMs) {
      const gapMs = elapsed - this.intervalMs
      this.gapCount += 1
      this.totalGapMs += gapMs
      if (this.gapList.length < MAX_GAPS) this.gapList.push({ atMs: this.lastTick, gapMs })
    }
    this.lastTick = now
  }

  gaps(): readonly HostSuspendGap[] {
    return this.gapList
  }

  summary(): string | null {
    if (this.gapCount === 0) return null
    const longest = this.gapList.reduce((a, b) => (b.gapMs > a.gapMs ? b : a))
    return (
      `host process did not run for ${(this.totalGapMs / 1000).toFixed(1)} s ` +
      `(${this.gapCount} pause(s), longest ${(longest.gapMs / 1000).toFixed(1)} s at ` +
      `${new Date(longest.atMs).toISOString()}): system sleep, SIGSTOP or a blocked event loop. ` +
      `Wall-clock timeouts in this run are not a test result.`
    )
  }
}

let singleton: HostSuspendWatch | null = null

/** Idempotent. */
export function startHostSuspendWatch(): HostSuspendWatch {
  if (!singleton) {
    singleton = new HostSuspendWatch()
    singleton.start()
  }
  return singleton
}

export function hostSuspendSummary(): string | null {
  return singleton?.summary() ?? null
}
