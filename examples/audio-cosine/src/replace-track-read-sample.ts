/**
 * Audio streaming demo with **replaceTrack** and **readSample**.
 *
 * Builds on the basic cosine sender (`examples/audio-cosine/src/index.ts`) and adds:
 * - {@link RemoteAudioTrack.readSample} — Opus RTP decoded to PCM in Rust
 * - {@link RTCRtpSender.replaceTrack} — swap outbound source without renegotiation
 *
 * Flow:
 * 1. Peer A streams 440 Hz tone via `addTrack`
 * 2. Peer B receives track, calls `readSample()` for decoded PCM
 * 3. Peer A `replaceTrack()` to a second LocalAudioTrack streaming 880 Hz
 * 4. Peer B reads again (length should remain one 20 ms stereo frame)
 *
 * Run from repo root:
 *   npm run start:replace-track --workspace=@node-webrtc-rust/example-audio-cosine
 */

import { LocalAudioTrack, RemoteAudioTrack, RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { CosineStreamServer } from './cosine-stream-server.js'
import {
  attachPeerStateLoggers,
  DEMO_ICE_SERVERS,
  delay,
  logConnectionStats,
  waitForConnection,
} from '../../shared/webrtc-demo-helpers.js'
import {
  createKickFrame,
  PCM_FRAME_DURATION_MS,
  PCM_FULL_FRAME_BYTES,
  PCM_KICK_DURATION_MS,
  PCM_SAMPLE_RATE,
} from '../../shared/pcm-streaming.js'

const TONE_A_HZ = 440
const TONE_B_HZ = 880
const STREAM_SECONDS = 2
const REMOTE_TRACK_TIMEOUT_MS = 30_000

async function main(): Promise<void> {
  const server = new SignalingServer({ port: 8080 })
  await server.listen()
  console.log(`Signaling ws://localhost:${server.port}`)

  const sender = new RTCPeerConnection({ iceServers: DEMO_ICE_SERVERS })
  const receiver = new RTCPeerConnection({ iceServers: DEMO_ICE_SERVERS })
  attachPeerStateLoggers(sender, 'sender')
  attachPeerStateLoggers(receiver, 'receiver')

  const trackA = new LocalAudioTrack('tone-440', 'tone-stream')
  const trackB = new LocalAudioTrack('tone-880', 'tone-stream')
  const rtpSender = await sender.addTrack(trackA)

  const streamA = new CosineStreamServer({ frequencyHz: TONE_A_HZ, amplitude: 0.2 })
  const streamB = new CosineStreamServer({ frequencyHz: TONE_B_HZ, amplitude: 0.2 })
  streamA.subscribe(trackA)
  streamB.subscribe(trackB)

  const sigSender = new SignalingClient({
    url: `ws://localhost:${server.port}`,
    room: 'audio-replace',
    peerId: 'sender',
  })
  const sigReceiver = new SignalingClient({
    url: `ws://localhost:${server.port}`,
    room: 'audio-replace',
    peerId: 'receiver',
  })

  const cleanup = async (): Promise<void> => {
    streamA.stop()
    streamB.stop()
    sender.close()
    receiver.close()
    sigSender.disconnect()
    sigReceiver.disconnect()
    await server.close()
  }

  try {
    autoNegotiate({ pc: sender, signaling: sigSender, polite: false })
    autoNegotiate({ pc: receiver, signaling: sigReceiver, polite: true })

    const remoteTrackPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`no remote track within ${REMOTE_TRACK_TIMEOUT_MS / 1000}s`)),
        REMOTE_TRACK_TIMEOUT_MS,
      )
      receiver.ontrack = (event) => {
        clearTimeout(timer)
        if (event.track instanceof RemoteAudioTrack) {
          console.log('Receiver ontrack:', { id: event.track.id, kind: event.track.kind })
          resolve(event.track)
        } else {
          reject(new Error('expected RemoteAudioTrack'))
        }
      }
    })

    await sigSender.connect()
    await sigReceiver.connect()
    await waitForConnection(sender)
    await waitForConnection(receiver)

    // Prime RTP so the remote side fires ontrack (see examples/shared/pcm-streaming.ts).
    await trackA.writeSample(createKickFrame(), PCM_KICK_DURATION_MS)
    const remote = await remoteTrackPromise

    console.log(`Streaming ${TONE_A_HZ} Hz for ${STREAM_SECONDS}s…`)
    streamA.start()
    await delay(STREAM_SECONDS * 1000)
    streamA.stop()

    const pcmA = await remote.readSample()
    console.log(
      `readSample() after ${TONE_A_HZ} Hz: ${pcmA.length} bytes` +
        ` (expected ${PCM_FULL_FRAME_BYTES} for ${PCM_FRAME_DURATION_MS} ms @ ${PCM_SAMPLE_RATE} Hz stereo)`,
    )
    if (pcmA.length !== PCM_FULL_FRAME_BYTES) {
      throw new Error(`unexpected PCM length ${pcmA.length}`)
    }

    console.log(`replaceTrack → ${TONE_B_HZ} Hz (no renegotiation)`)
    await rtpSender.replaceTrack(trackB)
    if (rtpSender.track?.id !== 'tone-880') {
      throw new Error('replaceTrack did not attach tone-880')
    }

    console.log(`Streaming ${TONE_B_HZ} Hz for ${STREAM_SECONDS}s…`)
    streamB.start()
    await delay(STREAM_SECONDS * 1000)
    streamB.stop()

    const pcmB = await remote.readSample()
    console.log(`readSample() after ${TONE_B_HZ} Hz: ${pcmB.length} bytes`)

    await logConnectionStats(receiver, 'receiver')
    console.log('Done — closing peers')
    await cleanup()
    process.exit(0)
  } catch (error) {
    await cleanup().catch(() => undefined)
    throw error
  }
}

void main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : String(error))
  process.exit(1)
})
