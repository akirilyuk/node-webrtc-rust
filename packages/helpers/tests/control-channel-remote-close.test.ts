import { describe, expect, it } from 'vitest'

import { RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

type Channel = ReturnType<RTCPeerConnection['createDataChannel']>

function waitFor(predicate: () => boolean, timeoutMs: number, what: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const deadline = Date.now() + timeoutMs
    const tick = (): void => {
      if (predicate()) return resolve()
      if (Date.now() > deadline) return reject(new Error(`timed out waiting for ${what}`))
      setTimeout(tick, 20)
    }
    tick()
  })
}

/** Real native loopback (no mocks): the server-side channel must see the client's close. */
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
    try {
      const clientChannel = client.createDataChannel('voice-control')
      const serverChannelPromise = new Promise<Channel>((resolve) => {
        server.ondatachannel = (e) => resolve(e.channel)
      })
      autoNegotiate({ pc: client, signaling: sigClient, polite: false })
      autoNegotiate({ pc: server, signaling: sigServer, polite: true })
      await sigClient.connect()
      await sigServer.connect()

      const serverChannel = await serverChannelPromise
      await waitFor(
        () => serverChannel.readyState === 'open' && clientChannel.readyState === 'open',
        15_000,
        'both channels open',
      )

      let closedAt = 0
      const closed = new Promise<void>((resolve) => {
        serverChannel.onclose = () => {
          closedAt = Date.now()
          resolve()
        }
      })
      const t0 = Date.now()
      client.close()
      let timer: ReturnType<typeof setTimeout> | undefined
      try {
        await Promise.race([
          closed,
          new Promise<void>((_, reject) => {
            timer = setTimeout(() => reject(new Error('onclose not fired within 2000 ms')), 2_000)
          }),
        ])
      } finally {
        if (timer) clearTimeout(timer)
      }
      const elapsed = closedAt - t0
      console.log(`[remote-close] server-side onclose after ${elapsed}ms`)
      expect(elapsed).toBeLessThan(2_000)
    } finally {
      client.close()
      server.close()
      sigClient.disconnect()
      sigServer.disconnect()
      await signalingServer.close()
    }
  }, 30_000)
})
