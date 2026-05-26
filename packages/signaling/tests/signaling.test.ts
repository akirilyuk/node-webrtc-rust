import { afterAll, beforeAll, describe, expect, test } from 'vitest'

import { RTCPeerConnection } from '../../sdk/src'
import { waitForConnection, waitForOpen } from '../../sdk/tests/helpers'

import { autoNegotiate, SignalingClient, SignalingServer } from '../src'

const defaultIceConfig = {
  iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
}

describe('SignalingServer', () => {
  test('relays join notifications between peers', async () => {
    const server = new SignalingServer({ port: 0 })
    await server.listen(0)

    const clientA = new SignalingClient({
      url: `ws://localhost:${server.port}`,
      room: 'test',
      peerId: 'a',
    })
    const clientB = new SignalingClient({
      url: `ws://localhost:${server.port}`,
      room: 'test',
      peerId: 'b',
    })

    const joined = new Promise<string>((resolve) => {
      clientB.once('peer-joined', resolve)
    })

    await clientA.connect()
    await clientB.connect()

    await expect(joined).resolves.toBe('a')

    clientA.disconnect()
    clientB.disconnect()
    await server.close()
  })
})

describe('autoNegotiate', () => {
  let server: SignalingServer

  beforeAll(async () => {
    server = new SignalingServer({ port: 0 })
    await server.listen(0)
  })

  afterAll(async () => {
    await server.close()
  })

  test('exchanges offer, answer, and ICE until peers connect', async () => {
    const pc1 = new RTCPeerConnection(defaultIceConfig)
    const pc2 = new RTCPeerConnection(defaultIceConfig)

    const dc1 = pc1.createDataChannel('signaling-test')

    const sig1 = new SignalingClient({
      url: `ws://localhost:${server.port}`,
      room: 'auto-negotiate',
      peerId: 'pc1',
    })
    const sig2 = new SignalingClient({
      url: `ws://localhost:${server.port}`,
      room: 'auto-negotiate',
      peerId: 'pc2',
    })

    autoNegotiate({ pc: pc1, signaling: sig1, polite: false })
    autoNegotiate({ pc: pc2, signaling: sig2, polite: true })

    const dc2Promise = new Promise<typeof dc1>((resolve) => {
      pc2.ondatachannel = (event) => resolve(event.channel)
    })

    await sig1.connect()
    await sig2.connect()

    const dc2 = await dc2Promise

    await waitForOpen(dc1)
    await waitForOpen(dc2)
    await waitForConnection(pc1)
    await waitForConnection(pc2)

    expect(pc1.localDescription?.type).toBe('offer')
    expect(pc2.remoteDescription?.type).toBe('offer')
    expect(pc2.localDescription?.type).toBe('answer')
    expect(pc1.remoteDescription?.type).toBe('answer')
    expect(dc1.readyState).toBe('open')
    expect(dc2.readyState).toBe('open')

    pc1.close()
    pc2.close()
    sig1.disconnect()
    sig2.disconnect()
  })
})
