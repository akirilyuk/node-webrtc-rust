import { describe, expect, it } from 'vitest'

import { RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

/**
 * Real native loopback (no mocks), therefore `.integration`. Every phase is event-driven and
 * bounded, and a timeout names the phase together with the peer states. The 2026-10-11
 * follow-up "control channel cold start" was never reproduced; this makes the next occurrence
 * diagnosable instead of a bare 30 s vitest timeout.
 */

// Budgets: the sum (2 x CONNECT + DATACHANNEL + OPEN + CLOSE) plus one DUMP stays under the
// 30_000 ms test timeout, so the phase error always wins over vitest's own timeout.
const CONNECT_MS = 3_000
const DATACHANNEL_MS = 8_000
const OPEN_MS = 8_000
const CLOSE_MS = 2_000
const DUMP_MS = 2_000

type Channel = ReturnType<RTCPeerConnection['createDataChannel']>

async function statsSummary(pc: RTCPeerConnection): Promise<string> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    const report = await Promise.race([
      pc.getStats(),
      new Promise<null>((resolve) => {
        timer = setTimeout(() => resolve(null), DUMP_MS)
      }),
    ])
    if (!report) return 'stats=timeout'
    const parts: string[] = []
    for (const [id, s] of report) {
      if (s.type === 'candidate-pair') {
        parts.push(
          `pair(${id}) state=${String(s.state)} sent=${String(s.bytesSent)} recv=${String(s.bytesReceived)}`,
        )
      } else if (s.type === 'transport') {
        parts.push(`transport(${id}) dtls=${String(s.dtlsState)}`)
      }
    }
    return `stats=[${parts.join(' | ')}]`
  } catch (e) {
    return `stats=error(${String(e)})`
  } finally {
    if (timer) clearTimeout(timer)
  }
}

async function describePeers(
  client: RTCPeerConnection,
  server: RTCPeerConnection,
  channels: { client?: Channel; server?: Channel },
): Promise<string> {
  try {
    const out: string[] = []
    for (const [name, pc] of [
      ['client', client],
      ['server', server],
    ] as const) {
      out.push(
        `${name}: conn=${pc.connectionState} ice=${pc.iceConnectionState} gathering=${pc.iceGatheringState} signaling=${pc.signalingState} ${await statsSummary(pc)}`,
      )
    }
    out.push(
      `clientChannel=${channels.client?.readyState} serverChannel=${channels.server?.readyState}`,
    )
    return out.join('; ')
  } catch (e) {
    return `dump failed: ${String(e)}`
  }
}

async function withDeadline<T>(
  phase: string,
  ms: number,
  work: Promise<T>,
  failed: Promise<never>,
  dump: () => Promise<string>,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      work,
      failed,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => {
          dump().then(
            (d) => reject(new Error(`phase "${phase}" not done within ${ms} ms; ${d}`)),
            (e) =>
              reject(
                new Error(`phase "${phase}" not done within ${ms} ms; dump failed: ${String(e)}`),
              ),
          )
        }, ms)
      }),
    ])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

describe('control channel remote close (native loopback)', () => {
  it('client.close() fires onclose on the server-side data channel within 2000 ms', async () => {
    const signalingServer = new SignalingServer({ port: 0 })
    await signalingServer.listen(0, '127.0.0.1')
    const client = new RTCPeerConnection()
    const server = new RTCPeerConnection()
    const sigClient = new SignalingClient({
      url: `ws://127.0.0.1:${signalingServer.port}`,
      room: 'remote-close',
      peerId: 'client-1',
    })
    const sigServer = new SignalingClient({
      url: `ws://127.0.0.1:${signalingServer.port}`,
      room: 'remote-close',
      peerId: 'server-1',
    })
    const channels: { client?: Channel; server?: Channel } = {}
    const dump = (): Promise<string> => describePeers(client, server, channels)
    let closing = false
    const failed = new Promise<never>((_, reject) => {
      for (const [name, pc] of [
        ['client', client],
        ['server', server],
      ] as const) {
        pc.onconnectionstatechange = () => {
          const state = pc.connectionState
          if (state === 'failed' || (state === 'closed' && !closing)) {
            void dump().then((d) => reject(new Error(`${name} connectionState=${state}; ${d}`)))
          }
        }
      }
    })
    // An unobserved rejection after the test ends must not surface as unhandled.
    failed.catch(() => {})
    try {
      const clientChannel = client.createDataChannel('voice-control')
      channels.client = clientChannel
      const clientOpen = new Promise<void>((resolve) => {
        if (clientChannel.readyState === 'open') return resolve()
        clientChannel.onopen = () => resolve()
      })
      let serverOpen: Promise<void> = Promise.resolve()
      const serverChannelPromise = new Promise<Channel>((resolve) => {
        server.ondatachannel = (e) => {
          const ch = e.channel
          channels.server = ch
          serverOpen = new Promise<void>((res) => {
            if (ch.readyState === 'open') return res()
            ch.onopen = () => res()
          })
          resolve(ch)
        }
      })
      autoNegotiate({ pc: client, signaling: sigClient, polite: false })
      autoNegotiate({ pc: server, signaling: sigServer, polite: true })
      await withDeadline('client signaling connect', CONNECT_MS, sigClient.connect(), failed, dump)
      await withDeadline('server signaling connect', CONNECT_MS, sigServer.connect(), failed, dump)

      const serverChannel = await withDeadline(
        'server ondatachannel',
        DATACHANNEL_MS,
        serverChannelPromise,
        failed,
        dump,
      )
      await withDeadline(
        'both channels open',
        OPEN_MS,
        Promise.all([clientOpen, serverOpen]),
        failed,
        dump,
      )

      let closedAt = 0
      const closed = new Promise<void>((resolve) => {
        serverChannel.onclose = () => {
          closedAt = Date.now()
          resolve()
        }
      })
      const t0 = Date.now()
      closing = true
      client.close()
      await withDeadline('server onclose', CLOSE_MS, closed, failed, dump)
      const elapsed = closedAt - t0
      console.log(`[remote-close] server-side onclose after ${elapsed}ms`)
      expect(elapsed).toBeLessThan(2_000)
    } finally {
      closing = true
      client.close()
      server.close()
      sigClient.disconnect()
      sigServer.disconnect()
      await signalingServer.close()
    }
  }, 30_000)
})
