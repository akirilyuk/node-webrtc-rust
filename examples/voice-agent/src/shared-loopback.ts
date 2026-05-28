/**
 * Shared loopback peer connection setup for voice-agent examples.
 */

import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import {
  LocalAudioTrack,
  RTCPeerConnection,
  type RemoteAudioTrack,
  type RTCPeerConnection as RTCPeerConnectionType,
} from '@node-webrtc-rust/sdk'

import {
  createKickFrame,
  PCM_FRAME_DURATION_MS,
  PCM_FULL_FRAME_BYTES,
  PCM_KICK_DURATION_MS,
} from '../../shared/pcm-streaming.js'

const STUN = { urls: 'stun:stun.l.google.com:19302' }

export async function createLoopbackAudio(): Promise<{
  server: SignalingServer
  agentPc: RTCPeerConnectionType
  userPc: RTCPeerConnectionType
  agentOut: LocalAudioTrack
  agentInbound: RemoteAudioTrack
  userInbound: RemoteAudioTrack
  cleanup: () => Promise<void>
}> {
  const bidirectional = await createBidirectionalLoopback()
  return {
    server: bidirectional.server,
    agentPc: bidirectional.agentPc,
    userPc: bidirectional.userPc,
    agentOut: bidirectional.agentOut,
    agentInbound: bidirectional.agentInbound,
    userInbound: bidirectional.userInbound,
    cleanup: bidirectional.cleanup,
  }
}

/** Agent and user PCs with audio in both directions (for STT + TTS demos). */
export async function createBidirectionalLoopback(): Promise<{
  server: SignalingServer
  agentPc: RTCPeerConnectionType
  userPc: RTCPeerConnectionType
  agentOut: LocalAudioTrack
  agentInbound: RemoteAudioTrack
  userOut: LocalAudioTrack
  userInbound: RemoteAudioTrack
  cleanup: () => Promise<void>
}> {
  const server = new SignalingServer({ port: 0 })
  await server.listen(0)

  const agentPc = new RTCPeerConnection({ iceServers: [STUN] })
  const userPc = new RTCPeerConnection({ iceServers: [STUN] })
  const agentOut = new LocalAudioTrack('agent-out', 'voice-demo')
  const userOut = new LocalAudioTrack('user-out', 'voice-demo')
  await agentPc.addTrack(agentOut)
  await userPc.addTrack(userOut)

  const sigAgent = new SignalingClient({
    url: `ws://localhost:${server.port}`,
    room: 'voice-demo',
    peerId: 'agent',
  })
  const sigUser = new SignalingClient({
    url: `ws://localhost:${server.port}`,
    room: 'voice-demo',
    peerId: 'user',
  })

  autoNegotiate({ pc: agentPc, signaling: sigAgent, polite: false })
  autoNegotiate({ pc: userPc, signaling: sigUser, polite: true })

  const agentInboundPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('timed out waiting for agent ontrack')), 20_000)
    agentPc.ontrack = (event) => {
      if (event.track.kind === 'audio') {
        clearTimeout(timer)
        resolve(event.track as RemoteAudioTrack)
      }
    }
  })

  const userInboundPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('timed out waiting for user ontrack')), 20_000)
    userPc.ontrack = (event) => {
      if (event.track.kind === 'audio') {
        clearTimeout(timer)
        resolve(event.track as RemoteAudioTrack)
      }
    }
  })

  await sigAgent.connect()
  await sigUser.connect()

  await waitConnected(agentPc)
  await waitConnected(userPc)

  await agentOut.writeSample(createKickFrame(), PCM_KICK_DURATION_MS)
  await userOut.writeSample(createKickFrame(), PCM_KICK_DURATION_MS)

  const [agentInbound, userInbound] = await Promise.all([
    agentInboundPromise,
    userInboundPromise,
  ])

  return {
    server,
    agentPc,
    userPc,
    agentOut,
    agentInbound,
    userOut,
    userInbound,
    cleanup: async () => {
      agentPc.close()
      userPc.close()
      sigAgent.disconnect()
      sigUser.disconnect()
      await server.close()
    },
  }
}

/** Streams a 440 Hz tone on userOut for STT/VAD manual checks. */
export async function streamUserTone(
  userOut: LocalAudioTrack,
  seconds: number,
): Promise<void> {
  const frameCount = Math.ceil((seconds * 1000) / PCM_FRAME_DURATION_MS)
  for (let i = 0; i < frameCount; i++) {
    await userOut.writeSample(createToneFrame(440), PCM_FRAME_DURATION_MS)
  }
}

function createToneFrame(hz: number): Buffer {
  const buf = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  const samplesPerChannel = PCM_FULL_FRAME_BYTES / 4
  for (let i = 0; i < samplesPerChannel; i++) {
    const t = i / 48_000
    const sample = Math.sin(2 * Math.PI * hz * t) * 16_000
    const clamped = Math.max(-32768, Math.min(32767, Math.floor(sample)))
    buf.writeInt16LE(clamped, i * 4)
    buf.writeInt16LE(clamped, i * 4 + 2)
  }
  return buf
}

async function waitConnected(pc: RTCPeerConnectionType): Promise<void> {
  if (pc.connectionState === 'connected') return
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('connection timeout')), 20_000)
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === 'connected') {
        clearTimeout(timer)
        resolve()
      }
    }
  })
}

export const mockVoiceConfig = {
  stt: { provider: 'mock' as const, language: 'en' },
  tts: { provider: 'mock' as const, voice: 'demo' },
}
