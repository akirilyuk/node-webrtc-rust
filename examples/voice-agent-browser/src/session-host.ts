/**
 * Node-side VoiceAgent host for one browser client per signaling room.
 *
 * Per client we negotiate:
 * - **Outbound** agent TTS track → browser `<audio>`
 * - **Inbound** browser mic → VAD + STT
 * - **`voice-control` DataChannel** → speech events down, `{ type: 'speak' }` up
 */

import {
  LocalAudioTrack,
  RTCIceCandidate,
  RTCPeerConnection,
  RTCSessionDescription,
  type RemoteAudioTrack,
  type RTCDataChannel,
  type RTCIceCandidateInit,
  type RTCSessionDescriptionInit,
} from '@node-webrtc-rust/sdk'
import {
  VoiceAgent,
  VOICE_CONTROL_CHANNEL_LABEL,
  forwardVoiceAgentSpeechToDataChannel,
  wireVoiceAgentToDataChannel,
  type VoiceAgentConfig,
} from '@node-webrtc-rust/sdk/voice'
import type { SignalingClient } from '@node-webrtc-rust/signaling'

import {
  createKickFrame,
  PCM_KICK_DURATION_MS,
} from '../../shared/pcm-streaming.js'

export const SERVER_PEER_ID = 'voice-agent-server'

interface IceServerConfig {
  urls: string | string[]
}

interface ClientSession {
  pc: RTCPeerConnection
  agentOut: LocalAudioTrack
  controlChannel: RTCDataChannel
  agent: VoiceAgent
  inboundTrack?: RemoteAudioTrack
  agentStarted: boolean
  unwireControl?: () => void
  unwireSpeechForward?: () => void
  remoteDescriptionSet: boolean
  offerSent: boolean
  pendingAnswer: RTCSessionDescriptionInit | null
  pendingIce: RTCIceCandidateInit[]
}

export interface VoiceAgentSessionHostOptions {
  voiceConfig: VoiceAgentConfig
}

/**
 * Impolite server: creates offer + outbound data channel for each joining browser client.
 */
export class VoiceAgentSessionHost {
  private readonly sessions = new Map<string, ClientSession>()

  constructor(
    private readonly signaling: SignalingClient,
    private readonly iceServers: IceServerConfig[],
    private readonly options: VoiceAgentSessionHostOptions,
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
    const agentOut = new LocalAudioTrack(`agent-out-${peerId}`, 'voice-agent')
    const agent = new VoiceAgent(this.options.voiceConfig)

    // Server creates the control channel before the offer (browser receives via ondatachannel).
    const controlChannel = pc.createDataChannel(VOICE_CONTROL_CHANNEL_LABEL, { ordered: true })
    await pc.addTrack(agentOut)

    const session: ClientSession = {
      pc,
      agentOut,
      controlChannel,
      agent,
      agentStarted: false,
      remoteDescriptionSet: false,
      offerSent: false,
      pendingAnswer: null,
      pendingIce: [],
    }
    this.sessions.set(peerId, session)

    const inboundPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`timed out waiting for mic track from ${peerId}`)),
        30_000,
      )
      pc.ontrack = (event) => {
        if (event.track.kind === 'audio') {
          clearTimeout(timer)
          resolve(event.track as RemoteAudioTrack)
        }
      }
    })

    pc.onicecandidate = (event) => {
      if (event.candidate) {
        this.signaling.sendIceCandidate(peerId, event.candidate.toJSON())
      }
    }

    pc.onconnectionstatechange = () => {
      console.log(`[voice ${peerId}] connectionState=${pc.connectionState}`)
      if (pc.connectionState === 'connected') {
        void this.startAgentSession(peerId, inboundPromise).catch((error: unknown) => {
          console.error(`Failed to start VoiceAgent for ${peerId}:`, error)
        })
      } else if (pc.connectionState === 'failed') {
        this.closeClient(peerId)
      }
    }

    controlChannel.onopen = () => {
      console.log(`[voice ${peerId}] control channel open`)
      session.unwireControl?.()
      session.unwireControl = wireVoiceAgentToDataChannel(agent, controlChannel)
    }

    const offer = await pc.createOffer()
    await pc.setLocalDescription(offer)
    await pc.gatheringComplete()
    this.signaling.sendOffer(peerId, pc.localDescription!.toJSON())
    session.offerSent = true
    console.log(`[voice ${peerId}] offer sent (audio + ${VOICE_CONTROL_CHANNEL_LABEL} DC)`)

    if (session.pendingAnswer) {
      const sdp = session.pendingAnswer
      session.pendingAnswer = null
      await this.applyAnswer(peerId, sdp)
    }
  }

  private async startAgentSession(
    peerId: string,
    inboundPromise: Promise<RemoteAudioTrack>,
  ): Promise<void> {
    const session = this.sessions.get(peerId)
    if (!session || session.agentStarted) return
    session.agentStarted = true

    session.inboundTrack = await inboundPromise
    await session.agent.attach({
      inboundTrack: session.inboundTrack,
      outboundTrack: session.agentOut,
    })
    await session.agent.start()

    // Pull stream is active only after start — forward STT/VAD events to the browser.
    session.unwireSpeechForward?.()
    session.unwireSpeechForward = forwardVoiceAgentSpeechToDataChannel(
      session.agent,
      session.controlChannel,
    )

    // Prime outbound RTP so the browser decoder starts before TTS PCM arrives.
    await session.agentOut.writeSample(createKickFrame(), PCM_KICK_DURATION_MS)
    console.log(`[voice ${peerId}] VoiceAgent started — mic → STT, TTS → browser`)
  }

  private async onAnswerReceived(
    peerId: string,
    sdp: RTCSessionDescriptionInit,
  ): Promise<void> {
    const session = this.sessions.get(peerId)
    if (!session) {
      console.warn(`[voice ${peerId}] answer received but no session yet`)
      return
    }
    if (!session.offerSent) {
      session.pendingAnswer = sdp
      return
    }
    await this.applyAnswer(peerId, sdp)
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
        `[voice ${peerId}] answer applied, connectionState=${session.pc.connectionState}`,
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
    session.unwireControl?.()
    session.unwireSpeechForward?.()
    void session.agent.stop().catch(() => undefined)
    session.pc.close()
    this.sessions.delete(peerId)
  }
}
