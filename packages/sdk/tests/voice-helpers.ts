import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { LocalAudioTrack, RTCPeerConnection, type RemoteAudioTrack } from '../src'

const STUN = { urls: 'stun:stun.l.google.com:19302' }

export async function createVoiceLoopback(): Promise<{
  cleanup: () => Promise<void>
  agentOut: LocalAudioTrack
  userInbound: RemoteAudioTrack
}> {
  const server = new SignalingServer({ port: 0 })
  await server.listen(0)

  const agentPc = new RTCPeerConnection({ iceServers: [STUN] })
  const userPc = new RTCPeerConnection({ iceServers: [STUN] })
  const agentOut = new LocalAudioTrack('agent-out', 'voice-test')
  await agentPc.addTrack(agentOut)

  const sigAgent = new SignalingClient({
    url: `ws://localhost:${server.port}`,
    room: 'voice-test',
    peerId: 'agent',
  })
  const sigUser = new SignalingClient({
    url: `ws://localhost:${server.port}`,
    room: 'voice-test',
    peerId: 'user',
  })

  autoNegotiate({ pc: agentPc, signaling: sigAgent, polite: false })
  autoNegotiate({ pc: userPc, signaling: sigUser, polite: true })

  const inboundPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('timed out waiting for ontrack')), 20_000)
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
  await agentOut.writeSample(Buffer.alloc(960), 5)
  const userInbound = await inboundPromise

  return {
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

async function waitConnected(pc: RTCPeerConnection): Promise<void> {
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
  tts: { provider: 'mock' as const },
}
