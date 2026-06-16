/**
 * Two Node peers — binary ArrayBuffer round-trip on a DataChannel.
 *
 * Run: npm run start:binary --workspace=@node-webrtc-rust/example-peer-connection
 */

import { RTCPeerConnection, type RTCDataChannel } from '@node-webrtc-rust/sdk'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import {
  createStateBuffer,
  decodePlayerState,
  encodePlayerState,
  PLAYER_STATE_FRAME_BYTES,
} from '../../shared/game-state-sync.js'
import { DEMO_ICE_SERVERS, waitForConnection, waitForDataChannelOpen } from '../../shared/webrtc-demo-helpers.js'

const MESSAGE_TIMEOUT_MS = 30_000
const STRESS_KB = Number(process.env.BINARY_STRESS_KB ?? 0)

async function waitForBinary(channel: RTCDataChannel, timeoutMs: number): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new Error(`no binary message within ${timeoutMs / 1000}s`))
    }, timeoutMs)
    channel.binaryType = 'arraybuffer'
    channel.onmessage = (event) => {
      if (typeof event.data === 'string') return
      clearTimeout(timer)
      resolve(Buffer.isBuffer(event.data) ? event.data : Buffer.from(event.data as ArrayBuffer))
    }
  })
}

async function main(): Promise<void> {
  const server = new SignalingServer({ port: 8080 })
  await server.listen()

  const pc1 = new RTCPeerConnection({ iceServers: DEMO_ICE_SERVERS })
  const pc2 = new RTCPeerConnection({ iceServers: DEMO_ICE_SERVERS })
  const sig1 = new SignalingClient({ url: `ws://localhost:${server.port}`, room: 'binary', peerId: 'pc1' })
  const sig2 = new SignalingClient({ url: `ws://localhost:${server.port}`, room: 'binary', peerId: 'pc2' })

  const cleanup = async (): Promise<void> => {
    pc1.close()
    pc2.close()
    sig1.disconnect()
    sig2.disconnect()
    await server.close()
  }

  try {
    const dc1 = pc1.createDataChannel('game-sync')
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

    if (STRESS_KB > 0) {
      const payload = new Uint8Array(STRESS_KB * 1024)
      payload[0] = 0xab
      payload[payload.length - 1] = 0xcd
      dc1.send(payload)
      const received = await waitForBinary(dc2, MESSAGE_TIMEOUT_MS)
      if (received.length !== payload.length || received[0] !== 0xab || received.at(-1) !== 0xcd) {
        throw new Error(`stress payload mismatch (${received.length} bytes)`)
      }
      console.log(`Stress round-trip OK (${STRESS_KB} KiB)`)
    } else {
      const state = createStateBuffer()
      encodePlayerState(state.view, 0, {
        tick: 42,
        playerId: 7,
        x: 12.5,
        y: -3.25,
        rot: 1.57,
      })
      dc1.send(state.bytes)
      const received = await waitForBinary(dc2, MESSAGE_TIMEOUT_MS)
      if (received.length !== PLAYER_STATE_FRAME_BYTES) {
        throw new Error(`expected ${PLAYER_STATE_FRAME_BYTES} bytes, got ${received.length}`)
      }
      const decoded = decodePlayerState(received)
      if (decoded.tick !== 42 || decoded.playerId !== 7) {
        throw new Error(`decoded mismatch: ${JSON.stringify(decoded)}`)
      }
      console.log('Binary player-state round-trip OK:', decoded)
    }

    await cleanup()
    process.exit(0)
  } catch (error) {
    await cleanup().catch(() => undefined)
    throw error
  }
}

void main().catch((error: unknown) => {
  console.error(error)
  process.exitCode = 1
})
