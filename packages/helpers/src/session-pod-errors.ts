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
