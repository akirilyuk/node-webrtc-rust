import { afterAll, beforeAll, describe, expect, test } from 'vitest'

import { RTCPeerConnection } from '../../sdk/src'
import { waitForConnection, waitForOpen } from '../../sdk/tests/helpers'

import {
  autoNegotiate,
  ConnectionError,
  setRootConnectionErrorHandler,
  SignalingClient,
  SignalingServer,
} from '../src'

/** Loopback peers: no STUN/TURN (hostname lookup can stall gathering on CI). */
const defaultIceConfig = {
  iceServers: [],
}

describe('SignalingServer', () => {
  test('relays join notifications between peers', async () => {
    const server = new SignalingServer({ port: 0 })
    await server.listen(0, '127.0.0.1')

    const clientA = new SignalingClient({
      url: `ws://127.0.0.1:${server.port}`,
      room: 'test',
      peerId: 'a',
    })
    const clientB = new SignalingClient({
      url: `ws://127.0.0.1:${server.port}`,
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

describe('SignalingClient', () => {
  test('bubbles connect failures to the root handler as ConnectionError', async () => {
    const server = new SignalingServer({ port: 0 })
    await server.listen(0, '127.0.0.1')
    const port = server.port

    const rootErrors: ConnectionError[] = []
    setRootConnectionErrorHandler((error) => rootErrors.push(error))

    const client = new SignalingClient({
      url: `ws://127.0.0.1:${port}`,
      room: 'error-after-connect',
      peerId: 'a',
    })
    await client.connect()
    client.disconnect()
    await server.close()

    const deadClient = new SignalingClient({
      url: `ws://127.0.0.1:${port}`,
      room: 'dead',
      peerId: 'b',
    })
    await expect(deadClient.connect()).rejects.toThrow()

    expect(rootErrors.length).toBeGreaterThan(0)
    expect(
      rootErrors.some(
        (error) =>
          ConnectionError.is(error) &&
          error.source.subsystem === 'signaling' &&
          error.source.phase === 'connect',
      ),
    ).toBe(true)

    setRootConnectionErrorHandler(undefined)
  })
})

describe('autoNegotiate', () => {
  let server: SignalingServer

  beforeAll(async () => {
    server = new SignalingServer({ port: 0 })
    await server.listen(0, '127.0.0.1')
  })

  afterAll(async () => {
    await server.close()
  })

  test('exchanges offer, answer, and ICE until peers connect', async () => {
    const pc1 = new RTCPeerConnection(defaultIceConfig)
    const pc2 = new RTCPeerConnection(defaultIceConfig)

    const dc1 = pc1.createDataChannel('signaling-test')

    const sig1 = new SignalingClient({
      url: `ws://127.0.0.1:${server.port}`,
      room: 'auto-negotiate',
      peerId: 'pc1',
    })
    const sig2 = new SignalingClient({
      url: `ws://127.0.0.1:${server.port}`,
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

  test('autoNegotiate opens a data channel whose native create completes 150 ms late', async () => {
    const pc1 = new RTCPeerConnection(defaultIceConfig)
    const pc2 = new RTCPeerConnection(defaultIceConfig)
    const sig1 = new SignalingClient({
      url: `ws://127.0.0.1:${server.port}`,
      room: 'auto-negotiate-late-native-dc',
      peerId: 'late-pc1',
    })
    const sig2 = new SignalingClient({
      url: `ws://127.0.0.1:${server.port}`,
      room: 'auto-negotiate-late-native-dc',
      peerId: 'late-pc2',
    })
    const withTimeout = <T>(promise: Promise<T>, name: string): Promise<T> => {
      let timer: ReturnType<typeof setTimeout> | undefined
      const timeout = new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error(`${name} did not happen within 8 s`)), 8_000)
      })
      return Promise.race([promise, timeout]).finally(() => clearTimeout(timer))
    }
    try {
      // Delay the impolite client's native create so the offer is built while it is in flight.
      const native = (
        pc1 as unknown as {
          native: { createDataChannel(label: string, options?: unknown): Promise<unknown> }
        }
      ).native
      const original = native.createDataChannel.bind(native)
      native.createDataChannel = (label, options) =>
        new Promise((resolve) => setTimeout(resolve, 150)).then(() => original(label, options))

      const dc1 = pc1.createDataChannel('voice-control')
      autoNegotiate({ pc: pc1, signaling: sig1, polite: false })
      autoNegotiate({ pc: pc2, signaling: sig2, polite: true })

      const dc2Promise = new Promise<typeof dc1>((resolve) => {
        pc2.ondatachannel = (event) => resolve(event.channel)
      })

      await sig1.connect()
      await sig2.connect()

      const dc2 = await withTimeout(dc2Promise, 'server ondatachannel')
      await withTimeout(waitForOpen(dc1, 8_000), 'client data channel open')
      await withTimeout(waitForOpen(dc2, 8_000), 'server data channel open')
      expect(dc1.readyState).toBe('open')
      expect(dc2.readyState).toBe('open')
    } finally {
      pc1.close()
      pc2.close()
      sig1.disconnect()
      sig2.disconnect()
    }
  })
})
