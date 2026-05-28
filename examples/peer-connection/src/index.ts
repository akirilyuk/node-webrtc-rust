/**
 * Minimal two-peer **DataChannel** demo.
 *
 * Shows the baseline WebRTC path before the v0.2 parity APIs:
 * - {@link RTCPeerConnection} + `@node-webrtc-rust/signaling` (`autoNegotiate`)
 * - SCTP data channel open/send/receive
 *
 * For transceivers, `getStats`, `setConfiguration`, `replaceTrack`, and `readSample`,
 * run the parity tour:
 *   npm run start:parity --workspace=@node-webrtc-rust/example-peer-connection
 *
 * From repo root:
 *   npm run start --workspace=@node-webrtc-rust/example-peer-connection
 */

import { RTCPeerConnection, type RTCDataChannel } from '@node-webrtc-rust/sdk'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import {
  attachPeerStateLoggers,
  DEMO_ICE_SERVERS,
  logConnectionStats,
  waitForConnection,
  waitForDataChannelOpen,
} from '../../shared/webrtc-demo-helpers.js'

const MESSAGE_TIMEOUT_MS = 30_000

async function waitForMessage(channel: RTCDataChannel, timeoutMs: number): Promise<string> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new Error(`no DataChannel message received within ${timeoutMs / 1000}s`))
    }, timeoutMs)

    channel.onmessage = (event) => {
      clearTimeout(timer)
      resolve(String(event.data))
    }
    channel.onerror = (event) => {
      clearTimeout(timer)
      reject(new Error(event.message ?? 'data channel error'))
    }
  })
}

async function main(): Promise<void> {
  const server = new SignalingServer({ port: 8080 })
  await server.listen()
  console.log(`Signaling ws://localhost:${server.port}`)

  const pc1 = new RTCPeerConnection({
    iceServers: DEMO_ICE_SERVERS,
    debug: process.env.WEBRTC_DEBUG === '1',
  })
  const pc2 = new RTCPeerConnection({
    iceServers: DEMO_ICE_SERVERS,
    debug: process.env.WEBRTC_DEBUG === '1',
  })

  // Parity: ICE/signaling state change callbacks (also available as EventEmitter events).
  attachPeerStateLoggers(pc1, 'pc1')
  attachPeerStateLoggers(pc2, 'pc2')

  const sig1 = new SignalingClient({
    url: `ws://localhost:${server.port}`,
    room: 'demo',
    peerId: 'pc1',
  })
  const sig2 = new SignalingClient({
    url: `ws://localhost:${server.port}`,
    room: 'demo',
    peerId: 'pc2',
  })

  const cleanup = async (): Promise<void> => {
    pc1.close()
    pc2.close()
    sig1.disconnect()
    sig2.disconnect()
    await server.close()
  }

  try {
    const dc1 = pc1.createDataChannel('chat')

    // Parity: backpressure — fired when bufferedAmount drops below threshold after a large send.
    dc1.bufferedAmountLowThreshold = 64 * 1024
    dc1.onbufferedamountlow = () => {
      console.log(`[pc1] bufferedamountlow bufferedAmount=${dc1.bufferedAmount}`)
    }

    const dc2Promise = new Promise<RTCDataChannel>((resolve) => {
      pc2.ondatachannel = (event) => resolve(event.channel)
    })

    autoNegotiate({ pc: pc1, signaling: sig1, polite: false })
    autoNegotiate({ pc: pc2, signaling: sig2, polite: true })

    await sig1.connect()
    await sig2.connect()

    const dc2 = await dc2Promise
    await waitForDataChannelOpen(dc1)
    await waitForDataChannelOpen(dc2)
    await waitForConnection(pc1)
    await waitForConnection(pc2)

    dc1.send('Hello from peer 1!')
    const payload = await waitForMessage(dc2, MESSAGE_TIMEOUT_MS)
    console.log('Received:', payload)

    // Parity: connection statistics (Map keyed by stat id, like browser RTCStatsReport).
    await logConnectionStats(pc1, 'pc1')
    await logConnectionStats(pc2, 'pc2')

    await cleanup()
    process.exit(0)
  } catch (error) {
    await cleanup().catch(() => undefined)
    throw error
  }
}

void main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error)
  console.error(message)
  process.exit(1)
})
