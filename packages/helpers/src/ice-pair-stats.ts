import type { StatsPeerConnection } from './inbound-audio-stats.js'

/** Default time to wait for the peer connection to produce a stats report. */
export const ICE_PAIR_STATS_TIMEOUT_MS = 500

/**
 * Summary of the ICE candidate pair a peer connection is using. Metadata only: candidate
 * addresses, ufrags and foundations are never read.
 */
export type SelectedIcePair =
  | {
      selected: true
      /** Local candidate type (`host`, `srflx`, `prflx`, `relay`) or `?` when unknown. */
      localType: string
      /** Remote candidate type or `?` when unknown. */
      remoteType: string
      /** Local candidate network type (for example `udp4`) or `?`. */
      protocol: string
      /** Relay transport, only when the local candidate is a relay. */
      relayProtocol?: string
      /** Local candidate port. */
      localPort?: number
      /** Candidate-pair state (`succeeded` for a selected pair). */
      state: string
      nominated: boolean
      packetsSent?: number
      packetsReceived?: number
      /** STUN binding requests sent on this pair. */
      requestsSent?: number
      /** STUN binding responses received on this pair. */
      responsesReceived?: number
      /** Milliseconds since the last packet was received on this pair. */
      lastRxAgeMs?: number
      /** Milliseconds since the last packet was sent on this pair. */
      lastTxAgeMs?: number
      /** Current round trip time in milliseconds. */
      rttMs?: number
    }
  | {
      selected: false
      /** Number of candidate pairs reported. */
      pairs: number
      /** Candidate pair count by state. */
      states: Record<string, number>
    }

type Stat = Record<string, unknown>

function finiteNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

function ageMs(timestampSeconds: unknown, nowMs: number): number | undefined {
  const ts = finiteNumber(timestampSeconds)
  if (ts === undefined || ts <= 0) return undefined
  return Math.round(nowMs - ts * 1000)
}

/**
 * Reads the selected ICE candidate pair from a peer connection.
 *
 * The selected pair is the `candidate-pair` that is `nominated` and in state `succeeded`; on a
 * tie the pair with the latest received packet wins. When none qualifies, resolves
 * `{ selected: false, pairs, states }`. Never rejects: a missing or closed connection, a
 * `getStats()` error or a timeout all resolve `null`.
 */
export async function readSelectedIcePair(
  pc: StatsPeerConnection | undefined,
  options: { timeoutMs?: number; nowMs?: number } = {},
): Promise<SelectedIcePair | null> {
  if (!pc || pc.connectionState === 'closed') return null
  const timeoutMs = options.timeoutMs ?? ICE_PAIR_STATS_TIMEOUT_MS
  const nowMs = options.nowMs ?? Date.now()
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    const timeout = new Promise<null>((resolve) => {
      timer = setTimeout(() => resolve(null), timeoutMs)
    })
    const report = await Promise.race([pc.getStats(), timeout])
    if (!report) return null
    const pairs: Stat[] = []
    const candidates = new Map<string, Stat>()
    for (const [key, stat] of report.entries()) {
      const row = stat as Stat
      if (row.type === 'candidate-pair') {
        pairs.push(row)
      } else if (row.type === 'local-candidate' || row.type === 'remote-candidate') {
        candidates.set(key, row)
        if (typeof row.id === 'string') candidates.set(row.id, row)
      }
    }
    let best: Stat | undefined
    let bestRx = -Infinity
    for (const row of pairs) {
      if (row.nominated !== true || row.state !== 'succeeded') continue
      const rx = finiteNumber(row.lastPacketReceivedTimestamp) ?? 0
      if (best === undefined || rx > bestRx) {
        best = row
        bestRx = rx
      }
    }
    if (!best) {
      const states: Record<string, number> = {}
      for (const row of pairs) {
        const state = typeof row.state === 'string' ? row.state : '?'
        states[state] = (states[state] ?? 0) + 1
      }
      return { selected: false, pairs: pairs.length, states }
    }
    const local =
      typeof best.localCandidateId === 'string' ? candidates.get(best.localCandidateId) : undefined
    const remote =
      typeof best.remoteCandidateId === 'string'
        ? candidates.get(best.remoteCandidateId)
        : undefined
    const localType = typeof local?.candidateType === 'string' ? local.candidateType : '?'
    const result: SelectedIcePair = {
      selected: true,
      localType,
      remoteType: typeof remote?.candidateType === 'string' ? remote.candidateType : '?',
      protocol: typeof local?.networkType === 'string' ? local.networkType : '?',
      state: typeof best.state === 'string' ? best.state : '?',
      nominated: true,
    }
    if (localType === 'relay' && typeof local?.relayProtocol === 'string') {
      result.relayProtocol = local.relayProtocol
    }
    const localPort = finiteNumber(local?.port)
    if (localPort !== undefined) result.localPort = localPort
    const packetsSent = finiteNumber(best.packetsSent)
    if (packetsSent !== undefined) result.packetsSent = packetsSent
    const packetsReceived = finiteNumber(best.packetsReceived)
    if (packetsReceived !== undefined) result.packetsReceived = packetsReceived
    const requestsSent = finiteNumber(best.requestsSent)
    if (requestsSent !== undefined) result.requestsSent = requestsSent
    const responsesReceived = finiteNumber(best.responsesReceived)
    if (responsesReceived !== undefined) result.responsesReceived = responsesReceived
    const lastRxAgeMs = ageMs(best.lastPacketReceivedTimestamp, nowMs)
    if (lastRxAgeMs !== undefined) result.lastRxAgeMs = lastRxAgeMs
    const lastTxAgeMs = ageMs(best.lastPacketSentTimestamp, nowMs)
    if (lastTxAgeMs !== undefined) result.lastTxAgeMs = lastTxAgeMs
    const rtt = finiteNumber(best.currentRoundTripTime)
    if (rtt !== undefined && rtt > 0) result.rttMs = Math.round(rtt * 1000)
    return result
  } catch {
    return null
  } finally {
    if (timer !== undefined) clearTimeout(timer)
  }
}

/**
 * Formats a {@link SelectedIcePair} as one log line of `key=value` pairs, for example
 * `ice_pair local=relay remote=srflx protocol=udp4 state=succeeded nominated=true`.
 * Contains no addresses.
 */
export function formatIcePairLine(p: SelectedIcePair): string {
  if (!p.selected) {
    const states = Object.keys(p.states)
      .sort()
      .map((k) => `${k}:${p.states[k]}`)
      .join(',')
    return `ice_pair none pairs=${p.pairs} states=${states}`
  }
  const parts = [`ice_pair local=${p.localType} remote=${p.remoteType} protocol=${p.protocol}`]
  if (p.relayProtocol !== undefined) parts.push(`relay_protocol=${p.relayProtocol}`)
  if (p.localPort !== undefined) parts.push(`local_port=${p.localPort}`)
  parts.push(`state=${p.state} nominated=${p.nominated}`)
  const counters: [string, number | undefined][] = [
    ['pkts_tx', p.packetsSent],
    ['pkts_rx', p.packetsReceived],
    ['stun_req_tx', p.requestsSent],
    ['stun_resp_rx', p.responsesReceived],
    ['last_rx_ms', p.lastRxAgeMs],
    ['last_tx_ms', p.lastTxAgeMs],
    ['rtt_ms', p.rttMs],
  ]
  for (const [name, value] of counters) {
    if (value !== undefined) parts.push(`${name}=${value}`)
  }
  return parts.join(' ')
}
