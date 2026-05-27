import {
  LocalAudioTrack,
  RTCIceCandidate,
  RTCPeerConnection,
  RTCSessionDescription,
  type RTCIceCandidateInit,
  type RTCSessionDescriptionInit,
} from '@node-webrtc-rust/sdk'
import type { SignalingClient } from '@node-webrtc-rust/signaling'

import {
  CosineGenerator,
  PCM_FRAME_DURATION_MS,
  PCM_KICK_DURATION_MS,
  PCM_KICK_FRAME_BYTES,
} from './cosine-generator'
import { logToneTick, logToneDebug, summarizePcmFrame, TONE_STREAM_DEBUG } from './tone-stream-debug'

interface ClientSession {
  pc: RTCPeerConnection
  track: LocalAudioTrack
  streaming: boolean
  stopStreaming: (() => void) | null
  remoteDescriptionSet: boolean
  offerSent: boolean
  pendingAnswer: RTCSessionDescriptionInit | null
  pendingIce: RTCIceCandidateInit[]
}

export const SERVER_PEER_ID = 'cosine-server'

interface IceServerConfig {
  urls: string | string[]
}

export interface CosineRoomHostOptions {
  frequencyHz: number
  amplitude: number
}

/**
 * Node-side host for one signaling room.
 *
 * Per-client streaming loop (see `examples/shared/pcm-streaming.ts`):
 * 1. Wait for `connectionState === 'connected'`
 * 2. Prime — `writeSample(960 B, 5 ms)` kicks browser ontrack / first RTP
 * 3. Stream — `await writeSample(3840 B, 20 ms)` every 20 ms until disconnect
 */
export class CosineRoomHost {
  private readonly sessions = new Map<string, ClientSession>()

  constructor(
    private readonly signaling: SignalingClient,
    private readonly streamOptions: CosineRoomHostOptions,
    private readonly iceServers: IceServerConfig[],
  ) {
    this.signaling.on('peer-joined', (peerId) => {
      if (peerId === SERVER_PEER_ID || this.sessions.has(peerId)) return
      if (!peerId.startsWith('client-')) return
      void this.connectClient(peerId).catch((error: unknown) => {
        console.error(`Failed to connect client ${peerId}:`, error)
        this.closeClient(peerId)
      })
    })

    this.signaling.on('answer', ({ peerId, sdp }) => {
      void this.onAnswerReceived(peerId, sdp)
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

    const session: ClientSession = {
      pc,
      track,
      streaming: false,
      stopStreaming: null,
      remoteDescriptionSet: false,
      offerSent: false,
      pendingAnswer: null,
      pendingIce: [],
    }
    this.sessions.set(peerId, session)

    pc.onicecandidate = (event) => {
      if (event.candidate) {
        this.signaling.sendIceCandidate(peerId, event.candidate.toJSON())
      }
    }

    pc.onconnectionstatechange = () => {
      console.log(`[tone ${peerId}] server pc connectionState=${pc.connectionState}`)
      logToneDebug(`[tone ${peerId}] connectionState=${pc.connectionState}`)
      if (pc.connectionState === 'connected') {
        void this.startStreaming(peerId).catch((error: unknown) => {
          console.error(`Failed to start streaming to ${peerId}:`, error)
        })
      } else if (pc.connectionState === 'failed') {
        this.closeClient(peerId)
      }
    }

    pc.oniceconnectionstatechange = () => {
      console.log(`[tone ${peerId}] server pc iceConnectionState=${pc.iceConnectionState}`)
    }

    const offer = await pc.createOffer()
    await pc.setLocalDescription(offer)
    await pc.gatheringComplete()
    this.signaling.sendOffer(peerId, pc.localDescription!.toJSON())
    session.offerSent = true
    console.log(`[tone ${peerId}] offer sent`)

    if (session.pendingAnswer) {
      const sdp = session.pendingAnswer
      session.pendingAnswer = null
      await this.applyAnswer(peerId, sdp)
    }
  }

  private async onAnswerReceived(
    peerId: string,
    sdp: RTCSessionDescriptionInit,
  ): Promise<void> {
    const session = this.sessions.get(peerId)
    if (!session) {
      console.warn(`[tone ${peerId}] answer received but no session yet`)
      return
    }
    if (!session.offerSent) {
      console.log(`[tone ${peerId}] answer queued until offer is sent`)
      session.pendingAnswer = sdp
      return
    }
    await this.applyAnswer(peerId, sdp)
  }

  private async startStreaming(peerId: string): Promise<void> {
    const session = this.sessions.get(peerId)
    if (!session || session.streaming) return
    session.streaming = true

    const generator = new CosineGenerator({
      frequencyHz: this.streamOptions.frequencyHz,
      amplitude: this.streamOptions.amplitude,
    })

    let active = true
    session.stopStreaming = () => {
      active = false
    }

    console.log(`Streaming ${generator.frequencyHz} Hz tone to ${peerId}`)

    const kickFrame = Buffer.alloc(PCM_KICK_FRAME_BYTES)
    const kickStats = summarizePcmFrame(kickFrame)
    const kickStart = performance.now()
    try {
      await session.track.writeSample(kickFrame, PCM_KICK_DURATION_MS)
      logToneTick(peerId, 'kick', 0, kickStats, {
        writeMs: performance.now() - kickStart,
        connectionState: session.pc.connectionState,
        ok: true,
        durationMs: PCM_KICK_DURATION_MS,
      })
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error)
      logToneTick(peerId, 'kick', 0, kickStats, {
        writeMs: performance.now() - kickStart,
        connectionState: session.pc.connectionState,
        ok: false,
        error: message,
        durationMs: PCM_KICK_DURATION_MS,
      })
      console.error(`Kick writeSample failed for ${peerId}:`, error)
      return
    }

    let tick = 0
    while (active && session.pc.connectionState === 'connected') {
      tick++
      const frame = generator.nextFrame()
      const stats = summarizePcmFrame(frame)
      const writeStart = performance.now()
      try {
        await session.track.writeSample(frame, PCM_FRAME_DURATION_MS)
        logToneTick(peerId, 'stream', tick, stats, {
          writeMs: performance.now() - writeStart,
          connectionState: session.pc.connectionState,
          ok: true,
          durationMs: PCM_FRAME_DURATION_MS,
        })
      } catch (error: unknown) {
        const message = error instanceof Error ? error.message : String(error)
        logToneTick(peerId, 'stream', tick, stats, {
          writeMs: performance.now() - writeStart,
          connectionState: session.pc.connectionState,
          ok: false,
          error: message,
          durationMs: PCM_FRAME_DURATION_MS,
        })
        console.error(`writeSample failed for ${peerId} on tick ${tick}:`, error)
        break
      }

      await delay(PCM_FRAME_DURATION_MS)
    }

    if (TONE_STREAM_DEBUG) {
      logToneDebug(`[tone ${peerId}] stream loop ended after ${tick} ticks`)
    }
  }

  private async applyAnswer(peerId: string, sdp: RTCSessionDescriptionInit): Promise<void> {
    const session = this.sessions.get(peerId)
    if (!session) return
    try {
      await session.pc.setRemoteDescription(new RTCSessionDescription(sdp))
      session.remoteDescriptionSet = true
      for (const candidate of session.pendingIce) {
        await session.pc.addIceCandidate(new RTCIceCandidate(candidate))
      }
      session.pendingIce = []
      console.log(
        `[tone ${peerId}] answer applied, connectionState=${session.pc.connectionState} ice=${session.pc.iceConnectionState}`,
      )
    } catch (error: unknown) {
      console.error(`Failed to apply answer from ${peerId}:`, error)
      this.closeClient(peerId)
    }
  }

  private async addRemoteIce(peerId: string, candidate: RTCIceCandidateInit): Promise<void> {
    const session = this.sessions.get(peerId)
    if (!session || !candidate.candidate) return
    if (!session.remoteDescriptionSet) {
      session.pendingIce.push(candidate)
      return
    }
    await session.pc.addIceCandidate(new RTCIceCandidate(candidate))
  }

  private closeClient(peerId: string): void {
    const session = this.sessions.get(peerId)
    if (!session) return
    session.stopStreaming?.()
    session.pc.close()
    this.sessions.delete(peerId)
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}
