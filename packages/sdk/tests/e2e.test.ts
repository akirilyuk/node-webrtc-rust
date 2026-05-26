import { afterAll, beforeAll, describe, expect, test } from 'vitest'

import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { LocalAudioTrack, RTCPeerConnection } from '../src'
import { defaultIceConfig, waitForConnection, waitForMessage, waitForOpen } from './helpers'

describe('End-to-end peer connection', () => {
  let server: SignalingServer

  beforeAll(async () => {
    server = new SignalingServer({ port: 0 })
    await server.listen(0)
  })

  afterAll(async () => {
    await server.close()
  })

  test('two peers exchange DataChannel messages via signaling', async () => {
    const pc1 = new RTCPeerConnection(defaultIceConfig)
    const pc2 = new RTCPeerConnection(defaultIceConfig)

    const dc1 = pc1.createDataChannel('test')

    const sig1 = new SignalingClient({
      url: `ws://localhost:${server.port}`,
      room: 'e2e-dc',
      peerId: 'pc1',
    })
    const sig2 = new SignalingClient({
      url: `ws://localhost:${server.port}`,
      room: 'e2e-dc',
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

    dc1.send('hello from pc1')
    const message = await waitForMessage(dc2)
    expect(message.data).toBe('hello from pc1')

    pc1.close()
    pc2.close()
    sig1.disconnect()
    sig2.disconnect()
  })

  test('two peers exchange an audio track via signaling', async () => {
    const pc1 = new RTCPeerConnection(defaultIceConfig)
    const pc2 = new RTCPeerConnection(defaultIceConfig)

    const localTrack = new LocalAudioTrack('audio-1', 'stream-1')
    await pc1.addTrack(localTrack)

    const sig1 = new SignalingClient({
      url: `ws://localhost:${server.port}`,
      room: 'e2e-audio',
      peerId: 'pc1',
    })
    const sig2 = new SignalingClient({
      url: `ws://localhost:${server.port}`,
      room: 'e2e-audio',
      peerId: 'pc2',
    })

    autoNegotiate({ pc: pc1, signaling: sig1, polite: false })
    autoNegotiate({ pc: pc2, signaling: sig2, polite: true })

    const remoteTrackPromise = new Promise<{ kind: string }>((resolve) => {
      pc2.ontrack = (event) => resolve({ kind: event.track.kind })
    })

    await sig1.connect()
    await sig2.connect()

    await waitForConnection(pc1)
    await waitForConnection(pc2)

    await localTrack.writeSample(Buffer.alloc(960))

    const remoteTrack = await remoteTrackPromise
    expect(remoteTrack.kind).toBe('audio')

    await waitForConnection(pc1)
    await waitForConnection(pc2)

    pc1.close()
    pc2.close()
    sig1.disconnect()
    sig2.disconnect()
  })
})
