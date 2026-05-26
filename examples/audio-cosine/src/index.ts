import { LocalAudioTrack, RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { PCM_FRAME_DURATION_MS, PCM_SAMPLE_RATE } from './cosine-generator'
import { CosineStreamServer } from './cosine-stream-server'

const ICE_SERVERS = [{ urls: 'stun:stun.l.google.com:19302' }]
const TONE_HZ = 440
const STREAM_SECONDS = 5

async function waitForConnection(pc: RTCPeerConnection, timeoutMs = 20_000): Promise<void> {
  if (pc.connectionState === 'connected') return

  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`connection timed out (state=${pc.connectionState})`)),
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

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

async function main(): Promise<void> {
  const server = new SignalingServer({ port: 8080 })
  await server.listen()
  console.log(`Signaling server listening on ws://localhost:${server.port}`)

  const sender = new RTCPeerConnection({ iceServers: ICE_SERVERS })
  const receiver = new RTCPeerConnection({ iceServers: ICE_SERVERS })

  const toneTrack = new LocalAudioTrack('cosine-tone', 'tone-stream')
  await sender.addTrack(toneTrack)

  const streamServer = new CosineStreamServer({ frequencyHz: TONE_HZ, amplitude: 0.2 })
  streamServer.subscribe(toneTrack)

  const sigSender = new SignalingClient({
    url: `ws://localhost:${server.port}`,
    room: 'audio-cosine',
    peerId: 'sender',
  })
  const sigReceiver = new SignalingClient({
    url: `ws://localhost:${server.port}`,
    room: 'audio-cosine',
    peerId: 'receiver',
  })

  autoNegotiate({ pc: sender, signaling: sigSender, polite: false })
  autoNegotiate({ pc: receiver, signaling: sigReceiver, polite: true })

  const remoteTrackPromise = new Promise<void>((resolve) => {
    receiver.ontrack = (event) => {
      console.log('Receiver got remote track:', {
        trackId: event.track.id,
        kind: event.track.kind,
        streamIds: event.streams.map((stream) => stream.id),
      })
      resolve()
    }
  })

  await sigSender.connect()
  await sigReceiver.connect()

  await waitForConnection(sender)
  await waitForConnection(receiver)
  console.log('Peers connected — starting audio stream')

  const totalFrames = Math.ceil((STREAM_SECONDS * 1000) / PCM_FRAME_DURATION_MS)
  console.log(
    `Streaming ${TONE_HZ} Hz tone: ${totalFrames} frames @ ${PCM_FRAME_DURATION_MS} ms (${PCM_SAMPLE_RATE} Hz stereo PCM)`,
  )

  await streamServer.primeTrack(toneTrack)
  await remoteTrackPromise

  streamServer.start()
  await delay(STREAM_SECONDS * 1000 - PCM_FRAME_DURATION_MS)
  streamServer.stop()

  console.log('Done — closing peers')
  sender.close()
  receiver.close()
  sigSender.disconnect()
  sigReceiver.disconnect()
  await server.close()
}

void main().catch((error: unknown) => {
  console.error(error)
  process.exitCode = 1
})
