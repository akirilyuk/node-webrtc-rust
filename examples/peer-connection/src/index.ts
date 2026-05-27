import { RTCPeerConnection, type RTCDataChannel } from '@node-webrtc-rust/sdk'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

const ICE_SERVERS = [{ urls: 'stun:stun.l.google.com:19302' }]
const MESSAGE_TIMEOUT_MS = 30_000

async function waitForOpen(channel: RTCDataChannel, timeoutMs = 15_000): Promise<void> {
  if (channel.readyState === 'open') return
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error('timed out waiting for data channel open')),
      timeoutMs,
    )
    channel.onopen = () => {
      clearTimeout(timer)
      resolve()
    }
    channel.onerror = (event) => {
      clearTimeout(timer)
      reject(new Error(event.message ?? 'data channel error'))
    }
  })
}

async function waitForConnection(pc: RTCPeerConnection, timeoutMs = 20_000): Promise<void> {
  if (pc.connectionState === 'connected') return
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`timed out (connectionState=${pc.connectionState})`)),
      timeoutMs,
    )
    const check = () => {
      if (pc.connectionState === 'connected') {
        clearTimeout(timer)
        resolve()
      } else if (pc.connectionState === 'failed' || pc.connectionState === 'closed') {
        clearTimeout(timer)
        reject(new Error(`connection ${pc.connectionState}`))
      }
    }
    pc.onconnectionstatechange = check
    check()
  })
}

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

  const pc1 = new RTCPeerConnection({ iceServers: ICE_SERVERS })
  const pc2 = new RTCPeerConnection({ iceServers: ICE_SERVERS })

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

    const dc2Promise = new Promise<RTCDataChannel>((resolve) => {
      pc2.ondatachannel = (event) => resolve(event.channel)
    })

    autoNegotiate({ pc: pc1, signaling: sig1, polite: false })
    autoNegotiate({ pc: pc2, signaling: sig2, polite: true })

    await sig1.connect()
    await sig2.connect()

    const dc2 = await dc2Promise
    await waitForOpen(dc1)
    await waitForOpen(dc2)
    await waitForConnection(pc1)
    await waitForConnection(pc2)

    dc1.send('Hello from peer 1!')
    const messagePromise = waitForMessage(dc2, MESSAGE_TIMEOUT_MS)
    const payload = await messagePromise

    console.log('Received:', payload)
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
