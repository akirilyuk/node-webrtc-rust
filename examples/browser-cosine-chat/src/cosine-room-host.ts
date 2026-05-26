import {
  LocalAudioTrack,
  RTCIceCandidate,
  RTCPeerConnection,
  RTCSessionDescription,
  type RTCIceCandidateInit,
  type RTCSessionDescriptionInit,
} from '@node-webrtc-rust/sdk'
import { SignalingClient } from '@node-webrtc-rust/signaling'

import { CosineStreamServer } from './cosine-stream-server'

interface ClientSession {
  pc: RTCPeerConnection
  track: LocalAudioTrack
  unsubscribe: () => void
}

export const SERVER_PEER_ID = 'cosine-server'

interface IceServerConfig {
  urls: string | string[]
}

/**
 * Node-side host for one signaling room.
 *
 * Creates a dedicated peer connection per browser client and streams the shared
 * cosine tone from {@link CosineStreamServer}.
 */
export class CosineRoomHost {
  private readonly sessions = new Map<string, ClientSession>()
  private readonly primedTracks = new Set<LocalAudioTrack>()

  constructor(
    private readonly signaling: SignalingClient,
    private readonly streamServer: CosineStreamServer,
    private readonly iceServers: IceServerConfig[],
  ) {
    this.signaling.on('peer-joined', (peerId) => {
      if (peerId === SERVER_PEER_ID || this.sessions.has(peerId)) return
      if (!peerId.startsWith('client-')) return
      void this.connectClient(peerId)
    })

    this.signaling.on('answer', ({ peerId, sdp }) => {
      void this.applyAnswer(peerId, sdp)
    })

    this.signaling.on('ice-candidate', ({ peerId, candidate }) => {
      void this.addRemoteIce(peerId, candidate)
    })

    this.signaling.on('peer-left', (peerId) => {
      this.closeClient(peerId)
    })
  }

  async close(): Promise<void> {
    for (const peerId of [...this.sessions.keys()]) {
      this.closeClient(peerId)
    }
  }

  private async connectClient(peerId: string): Promise<void> {
    const pc = new RTCPeerConnection({ iceServers: this.iceServers })
    const track = new LocalAudioTrack(`tone-${peerId}`, 'cosine-stream')
    await pc.addTrack(track)

    const unsubscribe = this.streamServer.subscribe(track)
    this.sessions.set(peerId, { pc, track, unsubscribe })

    pc.onicecandidate = (event) => {
      if (event.candidate) {
        this.signaling.sendIceCandidate(peerId, event.candidate.toJSON())
      }
    }

    pc.onconnectionstatechange = () => {
      if (pc.connectionState === 'connected' && !this.primedTracks.has(track)) {
        this.primedTracks.add(track)
        void this.streamServer.primeTrack(track)
      }
    }

    const offer = await pc.createOffer()
    await pc.setLocalDescription(offer)
    await pc.gatheringComplete()
    this.signaling.sendOffer(peerId, pc.localDescription!.toJSON())
  }

  private async applyAnswer(peerId: string, sdp: RTCSessionDescriptionInit): Promise<void> {
    const session = this.sessions.get(peerId)
    if (!session) return
    await session.pc.setRemoteDescription(new RTCSessionDescription(sdp))
  }

  private async addRemoteIce(peerId: string, candidate: RTCIceCandidateInit): Promise<void> {
    const session = this.sessions.get(peerId)
    if (!session || !candidate.candidate) return
    await session.pc.addIceCandidate(new RTCIceCandidate(candidate))
  }

  private closeClient(peerId: string): void {
    const session = this.sessions.get(peerId)
    if (!session) return
    this.primedTracks.delete(session.track)
    session.unsubscribe()
    session.pc.close()
    this.sessions.delete(peerId)
  }
}
