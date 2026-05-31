/**
 * WebRTC parity feature tour — Node.js CLI demo.
 *
 * Exercises APIs added on the parity branch (see `docs/webrtc-api-parity.md`):
 *
 * 1. **Unified Plan transceivers** — `addTransceiver`, `getTransceivers`, `getSenders`, `getReceivers`
 * 2. **Configuration** — `setConfiguration`, `getConfiguration`, `restartIce`
 * 3. **Media lifecycle** — `replaceTrack`, `removeTrack`, `RemoteAudioTrack.readSample`
 *
 * Run from repo root:
 *   npm run start:parity --workspace=@node-webrtc-rust/example-peer-connection
 *
 * Requires port 8080 free (signaling). Set `WEBRTC_DEBUG=1` for SDK trace logs.
 */

import { LocalAudioTrack, RemoteAudioTrack, RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import {
  attachPeerStateLoggers,
  DEMO_ICE_SERVERS,
  delay,
  logConnectionStats,
  logSection,
  waitForConnection,
  waitForDataChannelOpen,
  waitForIceGatheringComplete,
} from '../../shared/webrtc-demo-helpers.js'
import {
  createKickFrame,
  PCM_FRAME_DURATION_MS,
  PCM_FULL_FRAME_BYTES,
  PCM_KICK_DURATION_MS,
} from '../../shared/pcm-streaming.js'

const SIGNALING_PORT = 8080

async function withSignaling<T>(
  room: string,
  run: (server: SignalingServer) => Promise<T>,
): Promise<T> {
  const server = new SignalingServer({ port: SIGNALING_PORT })
  await server.listen()
  try {
    return await run(server)
  } finally {
    await server.close()
  }
}

function connectPair(
  server: SignalingServer,
  room: string,
  ids: [string, string],
): {
  pcA: RTCPeerConnection
  pcB: RTCPeerConnection
  sigA: SignalingClient
  sigB: SignalingClient
  cleanup: () => void
} {
  const pcA = new RTCPeerConnection({
    iceServers: DEMO_ICE_SERVERS,
    debug: process.env.WEBRTC_DEBUG === '1',
  })
  const pcB = new RTCPeerConnection({
    iceServers: DEMO_ICE_SERVERS,
    debug: process.env.WEBRTC_DEBUG === '1',
  })

  attachPeerStateLoggers(pcA, ids[0])
  attachPeerStateLoggers(pcB, ids[1])

  const sigA = new SignalingClient({
    url: `ws://localhost:${server.port}`,
    room,
    peerId: ids[0],
  })
  const sigB = new SignalingClient({
    url: `ws://localhost:${server.port}`,
    room,
    peerId: ids[1],
  })

  autoNegotiate({ pc: pcA, signaling: sigA, polite: false })
  autoNegotiate({ pc: pcB, signaling: sigB, polite: true })

  const cleanup = (): void => {
    pcA.close()
    pcB.close()
    sigA.disconnect()
    sigB.disconnect()
  }

  return { pcA, pcB, sigA, sigB, cleanup }
}

/** Scenario 1: callee uses recvonly transceiver instead of implicit recv from offer. */
async function demoTransceivers(): Promise<void> {
  logSection('addTransceiver (recvonly) + transceiver getters')

  await withSignaling('parity-transceiver', async (server) => {
    const { pcA, pcB, sigA, sigB, cleanup } = connectPair(server, 'parity-transceiver', [
      'sender',
      'receiver',
    ])

    const tone = new LocalAudioTrack('tone-a', 'stream-1')
    await pcA.addTrack(tone)

    // Unified Plan: declare receive intent before answer — matches browser `addTransceiver('audio', { direction: 'recvonly' })`.
    const rx = await pcB.addTransceiver('audio', { direction: 'recvonly' })
    console.log(`Receiver transceiver: kind=${rx.kind} direction=${rx.direction}`)

    const remoteTrackPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('ontrack timeout')), 30_000)
      pcB.ontrack = (event) => {
        clearTimeout(timer)
        if (event.track instanceof RemoteAudioTrack) {
          resolve(event.track)
        } else {
          reject(new Error(`expected RemoteAudioTrack, got ${event.track.constructor.name}`))
        }
      }
    })

    await sigA.connect()
    await sigB.connect()
    await waitForConnection(pcA)
    await waitForConnection(pcB)

    await tone.writeSample(createKickFrame(), PCM_KICK_DURATION_MS)
    const remote = await remoteTrackPromise
    console.log(`ontrack remote id=${remote.id} kind=${remote.kind}`)

    const transceivers = await pcB.getTransceivers()
    const senders = await pcB.getSenders()
    const receivers = await pcB.getReceivers()
    console.log(
      `getTransceivers=${transceivers.length} getSenders=${senders.length} getReceivers=${receivers.length}`,
    )
    console.log(
      `transceiver[0].mid=${transceivers[0]?.mid ?? '(pending)'} direction=${transceivers[0]?.direction}`,
    )

    await logConnectionStats(pcB, 'receiver')
    cleanup()
  })
}

/** Scenario 2: rotate ICE config mid-session; trigger ICE restart (offer uses iceRestart). */
async function demoConfigurationAndRestartIce(): Promise<void> {
  logSection('setConfiguration / getConfiguration / restartIce')

  await withSignaling('parity-config', async (server) => {
    const { pcA, pcB, sigA, sigB, cleanup } = connectPair(server, 'parity-config', [
      'cfg-a',
      'cfg-b',
    ])

    const dcA = pcA.createDataChannel('ping')
    const dcBPromise = new Promise<typeof dcA>((resolve) => {
      pcB.ondatachannel = (event) => resolve(event.channel)
    })

    await sigA.connect()
    await sigB.connect()
    await waitForConnection(pcA)
    await waitForConnection(pcB)
    const dcB = await dcBPromise
    await waitForDataChannelOpen(dcA)
    await waitForDataChannelOpen(dcB)
    console.log(`Data channels open: ${dcA.label} ↔ ${dcB.label}`)

    const before = pcA.getConfiguration()
    console.log(`getConfiguration iceServers=${before.iceServers?.length ?? 0}`)

    // Same STUN endpoint duplicated — demonstrates API without requiring a second TURN credential.
    await pcA.setConfiguration({
      iceServers: [...DEMO_ICE_SERVERS, { urls: 'stun:stun1.l.google.com:19302' }],
      iceTransportPolicy: 'all',
    })
    const after = pcA.getConfiguration()
    console.log(`after setConfiguration iceServers=${after.iceServers?.length ?? 0}`)

    await pcA.restartIce()
    console.log('restartIce() completed — wait for gathering before a follow-up offer')
    await waitForIceGatheringComplete(pcA)

    const offer = await pcA.createOffer({ iceRestart: true })
    console.log(`createOffer({ iceRestart: true }) type=${offer.type} sdpBytes=${offer.sdp.length}`)

    cleanup()
  })
}

/** Scenario 3: swap outbound track without renegotiation; decode inbound PCM; detach sender. */
async function demoReplaceRemoveAndReadSample(): Promise<void> {
  logSection('replaceTrack + readSample + removeTrack')

  await withSignaling('parity-media', async (server) => {
    const { pcA, pcB, sigA, sigB, cleanup } = connectPair(server, 'parity-media', [
      'media-a',
      'media-b',
    ])

    const trackA = new LocalAudioTrack('track-a', 'stream-media')
    const trackB = new LocalAudioTrack('track-b', 'stream-media')
    const sender = await pcA.addTrack(trackA)

    const remoteTrackPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('ontrack timeout')), 30_000)
      pcB.ontrack = (event) => {
        clearTimeout(timer)
        if (event.track instanceof RemoteAudioTrack) {
          resolve(event.track)
        } else {
          reject(new Error('expected RemoteAudioTrack'))
        }
      }
    })

    await sigA.connect()
    await sigB.connect()
    await waitForConnection(pcA)
    await waitForConnection(pcB)

    await trackA.writeSample(createKickFrame(), PCM_KICK_DURATION_MS)
    await trackA.writeSample(Buffer.alloc(PCM_FULL_FRAME_BYTES), PCM_FRAME_DURATION_MS)
    const remote = await remoteTrackPromise

    const pcm = await remote.readSample()
    console.log(`readSample() after track-a: ${pcm.length} bytes PCM (Opus decoded in Rust)`)

    await sender.replaceTrack(trackB)
    console.log(`replaceTrack → sender.track.id=${sender.track?.id ?? 'null'}`)
    expectTruthy(sender.track?.id === 'track-b', 'replaceTrack should point at track-b')

    await trackB.writeSample(Buffer.alloc(PCM_FULL_FRAME_BYTES), PCM_FRAME_DURATION_MS)
    const pcmAfterSwap = await remote.readSample()
    console.log(`readSample() after track-b: ${pcmAfterSwap.length} bytes`)

    await pcA.removeTrack(sender)
    console.log(`removeTrack() completed; sender.track=${sender.track?.id ?? 'null'}`)

    await delay(500)
    cleanup()
  })
}

function expectTruthy(condition: boolean, message: string): void {
  if (!condition) {
    throw new Error(message)
  }
}

async function main(): Promise<void> {
  console.log('WebRTC parity feature tour (Node.js)\n')

  await demoTransceivers()
  await demoConfigurationAndRestartIce()
  await demoReplaceRemoveAndReadSample()

  logSection('All parity scenarios completed')
  process.exit(0)
}

void main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : String(error))
  process.exit(1)
})
