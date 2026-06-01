/**
 * Process-wide cap on concurrent VoiceAgent + WebRTC connections.
 *
 * Used by {@link VoiceAgentSessionHost} and {@link SessionPod} so one Node worker
 * can enforce `VOICE_MAX_CONCURRENT_SESSIONS` from the environment or an injected limit.
 */

/** Snapshot for health endpoints and orchestrator hooks. */
export interface VoiceSessionBudgetSnapshot {
  /** Active slots (one per connected client peer). */
  active: number
  /** Configured maximum (`0` means unlimited). */
  max: number
  /** Slots still available (`Infinity` when unlimited). */
  available: number
  /** Cumulative rejections since process start. */
  rejectedTotal: number
}

export class VoiceSessionBudgetFullError extends Error {
  readonly name = 'VoiceSessionBudgetFullError'

  constructor(
    readonly snapshot: VoiceSessionBudgetSnapshot,
    readonly peerId?: string,
  ) {
    super(
      `voice session budget full (${snapshot.active}/${snapshot.max})` +
        (peerId ? ` — peer ${peerId}` : ''),
    )
  }
}

export interface VoiceSessionBudgetOptions {
  /**
   * Maximum concurrent voice connections in this process.
   * `0` or negative values mean unlimited.
   */
  maxSessions: number
}

let processBudget: VoiceSessionBudget | undefined

/**
 * Shared budget for this Node process (lazy-created from env on first use).
 */
export function getProcessVoiceSessionBudget(): VoiceSessionBudget {
  if (!processBudget) {
    processBudget = VoiceSessionBudget.fromEnv()
  }
  return processBudget
}

/** Reset the process singleton (tests only). */
export function resetProcessVoiceSessionBudget(): void {
  processBudget = undefined
}

export function resolveMaxVoiceSessionsFromEnv(env: NodeJS.ProcessEnv = process.env): number {
  const raw = env.VOICE_MAX_CONCURRENT_SESSIONS?.trim()
  if (!raw) return 0
  const parsed = Number(raw)
  if (!Number.isFinite(parsed) || parsed < 0) return 0
  return Math.floor(parsed)
}

/**
 * Limits how many {@link VoiceAgentSessionHost} client connections may be active at once.
 */
export class VoiceSessionBudget {
  private active = 0
  private rejectedTotal = 0
  private readonly slots = new Map<string, number>()

  constructor(private readonly maxSessions: number) {}

  static fromEnv(env: NodeJS.ProcessEnv = process.env): VoiceSessionBudget {
    return new VoiceSessionBudget(resolveMaxVoiceSessionsFromEnv(env))
  }

  get max(): number {
    return this.maxSessions
  }

  get isUnlimited(): boolean {
    return this.maxSessions <= 0
  }

  /**
   * Reserve a slot for `peerId`. Idempotent for the same peer (re-entrant connect).
   */
  tryAcquire(peerId: string): boolean {
    if (this.slots.has(peerId)) {
      return true
    }
    if (!this.isUnlimited && this.active >= this.maxSessions) {
      this.rejectedTotal += 1
      return false
    }
    this.slots.set(peerId, 1)
    this.active += 1
    return true
  }

  /**
   * Reserve a slot or throw {@link VoiceSessionBudgetFullError}.
   */
  acquire(peerId: string): void {
    if (!this.tryAcquire(peerId)) {
      throw new VoiceSessionBudgetFullError(this.snapshot(), peerId)
    }
  }

  /** Release a slot when the client disconnects. */
  release(peerId: string): void {
    if (!this.slots.delete(peerId)) return
    this.active = Math.max(0, this.active - 1)
  }

  snapshot(): VoiceSessionBudgetSnapshot {
    const max = this.isUnlimited ? 0 : this.maxSessions
    const available = this.isUnlimited
      ? Number.POSITIVE_INFINITY
      : Math.max(0, this.maxSessions - this.active)
    return {
      active: this.active,
      max,
      available,
      rejectedTotal: this.rejectedTotal,
    }
  }
}
