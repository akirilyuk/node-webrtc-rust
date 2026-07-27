/**
 * Process-wide cap on concurrent VoiceAgent + WebRTC connections.
 *
 * Used by {@link VoiceAgentSessionHost} and {@link SessionPod} so one Node worker
 * can enforce `VOICE_MAX_CONCURRENT_SESSIONS` from the environment or an injected limit.
 *
 * Leases are opaque tokens — not keyed by peerId. Each successful acquire returns a
 * unique lease; {@link release} requires that token. Cross-host / same-peerId sessions
 * each hold independent leases and each count toward capacity.
 */

/** Opaque lease id returned by {@link VoiceSessionBudget.tryAcquire}. */
export type VoiceSessionLease = string

/** Snapshot for health endpoints and orchestrator hooks. */
export interface VoiceSessionBudgetSnapshot {
  /** Active slots (one per outstanding lease). */
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
let nextLeaseSeq = 1

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

function mintLeaseId(): VoiceSessionLease {
  const seq = nextLeaseSeq++
  return `vlease-${seq}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`
}

/**
 * Limits how many {@link VoiceAgentSessionHost} client connections may be active at once.
 */
export class VoiceSessionBudget {
  private active = 0
  private rejectedTotal = 0
  /** Outstanding opaque leases. */
  private readonly leases = new Set<VoiceSessionLease>()

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
   * Reserve one capacity slot. Returns an opaque lease token, or `null` if full.
   * Not keyed by peerId — callers store the token on their session.
   */
  tryAcquire(_peerId?: string): VoiceSessionLease | null {
    if (!this.isUnlimited && this.active >= this.maxSessions) {
      this.rejectedTotal += 1
      return null
    }
    const lease = mintLeaseId()
    this.leases.add(lease)
    this.active += 1
    return lease
  }

  /**
   * Reserve a slot or throw {@link VoiceSessionBudgetFullError}.
   */
  acquire(peerId?: string): VoiceSessionLease {
    const lease = this.tryAcquire(peerId)
    if (lease == null) {
      throw new VoiceSessionBudgetFullError(this.snapshot(), peerId)
    }
    return lease
  }

  /**
   * Release a previously acquired lease. Unknown / already-released tokens are no-ops.
   */
  release(lease: VoiceSessionLease | null | undefined): void {
    if (lease == null) return
    if (!this.leases.delete(lease)) return
    this.active = Math.max(0, this.active - 1)
  }

  /** True when `lease` is currently held. */
  hasLease(lease: VoiceSessionLease): boolean {
    return this.leases.has(lease)
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
