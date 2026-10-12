/**
 * Connects several clients concurrently and never leaks one when another fails.
 *
 * `Promise.all` rejects on the first failure and drops the clients that did connect, so their
 * peer connections and signaling sockets stay open and the next test in the file reuses the
 * same ids and rooms against a half-torn-down pod. This waits for every attempt, closes the
 * ones that connected when any failed, then rethrows the first rejection.
 */
export async function connectAllOrCloseAll<T>(
  attempts: Array<() => Promise<T>>,
  close: (client: T) => void,
): Promise<T[]> {
  const settled = await Promise.allSettled(attempts.map((attempt) => attempt()))
  const clients: T[] = []
  let firstError: unknown
  let failed = false
  for (const result of settled) {
    if (result.status === 'fulfilled') {
      clients.push(result.value)
    } else if (!failed) {
      failed = true
      firstError = result.reason
    }
  }
  if (failed) {
    for (const client of clients) {
      try {
        close(client)
      } catch {
        // best effort: the connection failure is the error worth reporting
      }
    }
    throw firstError
  }
  return clients
}

/** `m=` kinds of an SDP, e.g. `audio,application`; `none` when the SDP is empty. */
export function sdpMediaKinds(sdp: string | undefined): string {
  const kinds = [...(sdp ?? '').matchAll(/^m=(\S+)/gm)].map((match) => match[1])
  return kinds.length > 0 ? kinds.join(',') : 'none'
}
