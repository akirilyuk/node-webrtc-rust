/** Thrown when {@link SessionPod.ensureSession} would exceed `maxPreparedSessions`. */
export class SessionPodCapacityFullError extends Error {
  readonly name = 'SessionPodCapacityFullError'

  constructor(
    readonly activeSlots: number,
    readonly maxSlots: number,
  ) {
    super(`session pod capacity full (${activeSlots}/${maxSlots})`)
  }
}

/**
 * Thrown when a host has quarantined native-close failures and the pod must not
 * accept new sessions until process recycle / quarantine clear.
 */
export class SessionPodRecycleRequiredError extends Error {
  readonly name = 'SessionPodRecycleRequiredError'

  constructor(readonly quarantined: number) {
    super(
      `session pod recycle required — ${quarantined} quarantined peer lease(s); orchestrator must not assign here`,
    )
  }
}
