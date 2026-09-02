import { afterAll, beforeAll, describe, expect, test } from 'vitest'

import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { LocalAudioTrack, RemoteAudioTrack, RTCPeerConnection } from '../src'
import { AudioMixGraph, quatIdentity, vec3Zero } from '../src/mix'
import { defaultIceConfig, delay, waitForConnection } from './helpers'
import {
  appendStereoChannels,
  assertTwoSinePanSides,
  FRAME_COUNT,
  sineStereoFrame,
  SAMPLES_PER_CHANNEL,
} from './mix-test-helpers'

// Same-process loopback: do not set WEBRTC_NAT_1TO1_IPS. That rewrite is for
// browser ↔ Node on a public NIC; in CI it left ICE in `connecting` until timeout.
// Match packages/sdk/tests/e2e.test.ts (defaultIceConfig + autoNegotiate).

interface PeerPairHandles {
  hostPc: RTCPeerConnection
  clientPc: RTCPeerConnection
  hostSig: SignalingClient
  clientSig: SignalingClient
}

async function connectPeerPair(
  wsUrl: string,
  room: string,
  clientId: string,
  role: 'client-sends' | 'host-sends',
): Promise<{
  pair: PeerPairHandles
  hostOutbound?: LocalAudioTrack
  hostInbound?: RemoteAudioTrack
  clientOutbound?: LocalAudioTrack
  clientInbound?: RemoteAudioTrack
}> {
  const hostPc = new RTCPeerConnection(defaultIceConfig)
  const clientPc = new RTCPeerConnection(defaultIceConfig)

  let hostOutbound: LocalAudioTrack | undefined
  let clientOutbound: LocalAudioTrack | undefined
  let hostInbound: RemoteAudioTrack | undefined
  let clientInbound: RemoteAudioTrack | undefined

  const senderPc = role === 'client-sends' ? clientPc : hostPc
  const receiverPc = role === 'client-sends' ? hostPc : clientPc
  const outbound = new LocalAudioTrack(
    role === 'client-sends' ? `${clientId}-out` : `${clientId}-host-out`,
    `stream-${clientId}`,
  )
  await senderPc.addTrack(outbound)
  if (role === 'client-sends') {
    clientOutbound = outbound
  } else {
    hostOutbound = outbound
  }

  const hostSig = new SignalingClient({
    url: wsUrl,
    room,
    peerId: `host-${clientId}`,
  })
  const clientSig = new SignalingClient({
    url: wsUrl,
    room,
    peerId: clientId,
  })

  const senderSig = role === 'client-sends' ? clientSig : hostSig
  const receiverSig = role === 'client-sends' ? hostSig : clientSig
  autoNegotiate({ pc: senderPc, signaling: senderSig, polite: false })
  autoNegotiate({ pc: receiverPc, signaling: receiverSig, polite: true })

  const remoteTrackPromise = new Promise<RemoteAudioTrack>((resolve) => {
    receiverPc.ontrack = (event) => {
      if (event.track instanceof RemoteAudioTrack) {
        resolve(event.track)
      }
    }
  })

  await senderSig.connect()
  await receiverSig.connect()
  await waitForConnection(senderPc)
  await waitForConnection(receiverPc)

  await outbound.writeSample(Buffer.alloc(960), 5)
  const inbound = await remoteTrackPromise
  if (role === 'client-sends') {
    hostInbound = inbound
  } else {
    clientInbound = inbound
  }

  return {
    pair: { hostPc, clientPc, hostSig, clientSig },
    hostOutbound,
    hostInbound,
    clientOutbound,
    clientInbound,
  }
}

function setupMixGraph(c1X: number, c3X: number): AudioMixGraph {
  const graph = new AudioMixGraph()
  for (const id of ['c1', 'c2', 'c3']) {
    graph.addInput(id)
  }
  graph.setPositionalEnabled(true)
  graph.setPose('c2', { position: vec3Zero(), orientation: quatIdentity() })
  graph.setPose('c1', {
    position: { ...vec3Zero(), x: c1X },
    orientation: quatIdentity(),
  })
  graph.setPose('c3', {
    position: { ...vec3Zero(), x: c3X },
    orientation: quatIdentity(),
  })
  graph.setListenerSources('c2', ['c1', 'c3'])
  return graph
}

function closePeerPair(pair: PeerPairHandles): void {
  pair.hostPc.close()
  pair.clientPc.close()
  pair.hostSig.disconnect()
  pair.clientSig.disconnect()
}

async function readSampleWithTimeout(
  track: RemoteAudioTrack,
  label: string,
  timeoutMs = 30_000,
): Promise<Buffer> {
  return Promise.race([
    track.readSample(),
    delay(timeoutMs).then(() => {
      throw new Error(`readSample timeout for ${label} after ${timeoutMs}ms`)
    }),
  ])
}

async function runThreePeerPositionalMix(
  wsUrl: string,
  runId: string,
  c1X: number,
  c3X: number,
  expect440OnRight: boolean,
): Promise<void> {
  const c1 = await connectPeerPair(wsUrl, `mix-three-${runId}-c1`, 'c1', 'client-sends')
  const c2 = await connectPeerPair(wsUrl, `mix-three-${runId}-c2`, 'c2', 'host-sends')
  const c3 = await connectPeerPair(wsUrl, `mix-three-${runId}-c3`, 'c3', 'client-sends')

  try {
    expect(c1.clientOutbound).toBeDefined()
    expect(c1.hostInbound).toBeDefined()
    expect(c2.hostOutbound).toBeDefined()
    expect(c2.clientInbound).toBeDefined()
    expect(c3.clientOutbound).toBeDefined()
    expect(c3.hostInbound).toBeDefined()

    const clientTrackC1 = c1.clientOutbound!
    const hostInboundC1 = c1.hostInbound!
    const hostOutboundC2 = c2.hostOutbound!
    const clientInboundC2 = c2.clientInbound!
    const clientTrackC3 = c3.clientOutbound!
    const hostInboundC3 = c3.hostInbound!

    const mixGraph = setupMixGraph(c1X, c3X)

    const phaseRefC1 = { value: 0 }
    const phaseRefC3 = { value: 0 }

    await clientTrackC1.writeSample(Buffer.alloc(960), 5)
    await clientTrackC3.writeSample(Buffer.alloc(960), 5)
    await hostOutboundC2.writeSample(Buffer.alloc(960), 5)

    const senders = (async () => {
      for (let i = 0; i < FRAME_COUNT + 4; i++) {
        await clientTrackC1.writeSample(sineStereoFrame(440, 10_000, phaseRefC1.value), 20)
        await clientTrackC3.writeSample(sineStereoFrame(880, 10_000, phaseRefC3.value), 20)
        phaseRefC1.value += SAMPLES_PER_CHANNEL
        phaseRefC3.value += SAMPLES_PER_CHANNEL
      }
    })()

    const left: number[] = []
    const right: number[] = []

    for (let i = 0; i < FRAME_COUNT; i++) {
      const pcm1 = await readSampleWithTimeout(hostInboundC1, 'hostInboundC1')
      const pcm3 = await readSampleWithTimeout(hostInboundC3, 'hostInboundC3')
      mixGraph.pushFrame('c1', pcm1)
      mixGraph.pushFrame('c3', pcm3)
      const mixed = mixGraph.renderOutput('c2')
      await hostOutboundC2.writeSample(mixed, 20)
      const pcm2 = await readSampleWithTimeout(clientInboundC2, 'clientInboundC2')
      appendStereoChannels(left, right, pcm2)
    }

    await senders

    assertTwoSinePanSides(Int16Array.from(left), Int16Array.from(right), expect440OnRight)
  } finally {
    closePeerPair(c1.pair)
    closePeerPair(c2.pair)
    closePeerPair(c3.pair)
    await delay(100)
  }
}

describe('three-peer WebRTC positional mix', () => {
  let server: SignalingServer
  let wsUrl: string

  beforeAll(async () => {
    server = new SignalingServer({ port: 0 })
    await server.listen(0)
    wsUrl = `ws://localhost:${server.port}`
  })

  afterAll(async () => {
    await server.close()
  })

  test('host mixes c1+c3 over WebRTC and c2 hears 440 Hz right / 880 Hz left', async () => {
    await runThreePeerPositionalMix(wsUrl, 'right', 3, -3, true)
  }, 90_000)

  test('swapped source poses flip stereo sides over three-peer WebRTC path', async () => {
    await runThreePeerPositionalMix(wsUrl, 'swap', -3, 3, false)
  }, 90_000)
})
