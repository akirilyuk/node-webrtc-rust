import type { RTCStatsReport } from '@node-webrtc-rust/sdk'

/** Default time to wait for the peer connection to produce a stats report. */
export const INBOUND_AUDIO_STATS_TIMEOUT_MS = 500

/** Audio received from one browser tab over its peer connection. */
export type InboundAudioStats = {
  /** RTP packets received on the inbound audio stream(s). */
  packetsReceived: number
  /** Payload bytes received on the inbound audio stream(s). */
  bytesReceived: number
}

/** Options for {@link VoiceSessionContext.getInboundAudioStats}. */
export type InboundAudioStatsOptions = {
  /** Give up and resolve `null` after this many milliseconds (default 500). */
  timeoutMs?: number
}

/** The slice of `RTCPeerConnection` that inbound audio stats need. */
export type StatsPeerConnection = {
  connectionState: string
  getStats(): Promise<RTCStatsReport>
}

/**
 * Reads inbound audio counters from a peer connection.
 *
 * Sums `packetsReceived` and `bytesReceived` over every `inbound-rtp` stat with
 * `kind === "audio"`. An entry whose counters are not both finite numbers is ignored.
 * Never rejects: a closed connection, a missing audio entry, a `getStats()` error or a
 * timeout all resolve `null`.
 */
export async function readInboundAudioStats(
  pc: StatsPeerConnection | undefined,
  options: InboundAudioStatsOptions = {},
): Promise<InboundAudioStats | null> {
  if (!pc || pc.connectionState === 'closed') return null
  const timeoutMs = options.timeoutMs ?? INBOUND_AUDIO_STATS_TIMEOUT_MS
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    const timeout = new Promise<null>((resolve) => {
      timer = setTimeout(() => resolve(null), timeoutMs)
    })
    const report = await Promise.race([pc.getStats(), timeout])
    if (!report) return null
    let packetsReceived = 0
    let bytesReceived = 0
    let found = false
    for (const stat of report.values()) {
      if (stat.type !== 'inbound-rtp' || stat.kind !== 'audio') continue
      const packets = stat.packetsReceived
      const bytes = stat.bytesReceived
      if (typeof packets !== 'number' || !Number.isFinite(packets)) continue
      if (typeof bytes !== 'number' || !Number.isFinite(bytes)) continue
      packetsReceived += packets
      bytesReceived += bytes
      found = true
    }
    return found ? { packetsReceived, bytesReceived } : null
  } catch {
    return null
  } finally {
    if (timer !== undefined) clearTimeout(timer)
  }
}
