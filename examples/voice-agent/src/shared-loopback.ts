/**
 * Shared loopback peer connection setup for voice-agent examples.
 *
 * Run: npm run start:callback --workspace=@node-webrtc-rust/example-voice-agent
 */

import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import {
  LocalAudioTrack,
  RTCPeerConnection,
  type RemoteAudioTrack,
  type RTCPeerConnection as RTCPeerConnectionType,
} from '@node-webrtc-rust/sdk'

const STUN = { urls: 'stun:stun.l.google.com:19302' }

export async function createLoopbackAudio(): Promise<{
  server: SignalingServer
  agentPc: RTCPeerConnectionType
  userPc: RTCPeerConnectionType
  agentOut: LocalAudioTrack
  userInbound: RemoteAudioTrack
  cleanup: () => Promise<void>
}> {
  const server = new SignalingServer({ port: 0 })
  await server.listen(0)

  const agentPc = new RTCPeerConnection({ iceServers: [STUN] })
  const userPc = new RTCPeerConnection({ iceServers: [STUN] })
  const agentOut = new LocalAudioTrack('agent-out', 'voice-demo')
  await agentPc.addTrack(agentOut)

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

  const inboundPromise = new Promise<RemoteAudioTrack>((resolve) => {
    userPc.ontrack = (event) => {
      if (event.track.kind === 'audio') {
        resolve(event.track as RemoteAudioTrack)
      }
    }
  })

  await sigAgent.connect()
  await sigUser.connect()

  await waitConnected(agentPc)
  await waitConnected(userPc)
  await agentOut.writeSample(Buffer.alloc(960), 5)
  const userInbound = await inboundPromise

  return {
    server,
    agentPc,
    userPc,
    agentOut,
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
