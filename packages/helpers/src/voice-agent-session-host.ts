/**
 * Node-side VoiceAgent host for browser clients in one signaling room.
 *
 * Per client we negotiate:
 * - **Outbound** agent TTS track → browser `<audio>`
 * - **Inbound** browser mic → VAD + STT
 * - **`voice-control` DataChannel** → speech events down, `{ type: 'speak' }` up
 *
 * Each joining `client-*` peer gets its own `RTCPeerConnection` and `VoiceAgent`.
 * Disconnect or PC failure stops the agent and closes the connection.
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
  speechEventToControlMessage,
  wireVoiceAgentToDataChannel,
  type SpeechEvent,
  type VoiceAgentConfig,
} from '@node-webrtc-rust/sdk/voice'
import type { SignalingClient } from '@node-webrtc-rust/signaling'

import { createKickFrame, PCM_KICK_DURATION_MS } from './pcm.js'
import {
  getProcessVoiceSessionBudget,
  type VoiceSessionBudget,
  type VoiceSessionBudgetSnapshot,
} from './voice-session-budget.js'
import type { VoiceSessionContext, VoiceSessionHandler } from './voice-session-handler.js'

export const VOICE_AGENT_SERVER_PEER_ID = 'voice-agent-server'

/** @deprecated Use {@link VOICE_AGENT_SERVER_PEER_ID}. */
export const SERVER_PEER_ID = VOICE_AGENT_SERVER_PEER_ID

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
  /** Peer id prefix for clients that receive an agent (default `client-`). */
  clientPeerIdPrefix?: string
  /** Log connection lifecycle when provided. */
  log?: (message: string) => void
  /**
   * Your app logic: react to STT/VAD events and send TTS replies.
   * See `examples/voice-agent-local-sherpa-multi-client/src/voice-handler.ts`.
   */
  voiceHandler?: VoiceSessionHandler
  /**
   * Process-wide connection cap (`VOICE_MAX_CONCURRENT_SESSIONS` when omitted).
   * Shared across rooms when using {@link SessionPod}.
   */
  sessionBudget?: VoiceSessionBudget
}

/**
 * Impolite server: creates offer + outbound data channel for each joining browser client.
 */
export class VoiceAgentSessionHost {
  private readonly sessions = new Map<string, ClientSession>()
  private readonly clientPeerIdPrefix: string
  private readonly log: (message: string) => void
  private readonly sessionBudget: VoiceSessionBudget

  constructor(
    private readonly signaling: SignalingClient,
    private readonly iceServers: IceServerConfig[],
    private readonly options: VoiceAgentSessionHostOptions,
  ) {
    this.clientPeerIdPrefix = options.clientPeerIdPrefix ?? 'client-'
    this.log = options.log ?? ((message) => console.log(message))
    this.sessionBudget = options.sessionBudget ?? getProcessVoiceSessionBudget()

    this.signaling.on('peer-joined', (peerId) => {
      if (peerId === VOICE_AGENT_SERVER_PEER_ID || this.sessions.has(peerId)) return
      if (!peerId.startsWith(this.clientPeerIdPrefix)) return
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

  /** Number of active browser clients (each owns one VoiceAgent + RTCPeerConnection). */
  get activeClientCount(): number {
    return this.sessions.size
  }

  /** Current process session budget (active / max / rejected). */
  get sessionBudgetSnapshot(): VoiceSessionBudgetSnapshot {
    return this.sessionBudget.snapshot()
  }

  async close(): Promise<void> {
    for (const peerId of [...this.sessions.keys()]) {
      this.closeClient(peerId)
    }
  }

  private async connectClient(peerId: string): Promise<void> {
    if (!this.sessionBudget.tryAcquire(peerId)) {
      const snap = this.sessionBudget.snapshot()
      this.log(
        `[voice ${peerId}] rejected — session budget full (${snap.active}/${snap.max}, rejectedTotal=${snap.rejectedTotal})`,
      )
      return
    }

    try {
      await this.connectClientInner(peerId)
    } catch (error: unknown) {
      this.closeClient(peerId)
      throw error
    }
  }

  private async connectClientInner(peerId: string): Promise<void> {
    const pc = new RTCPeerConnection({ iceServers: this.iceServers })
    const agentOut = new LocalAudioTrack(`agent-out-${peerId}`, 'voice-agent')
    const agent = new VoiceAgent(this.options.voiceConfig)

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
      this.log(`[voice ${peerId}] connectionState=${pc.connectionState}`)
      if (pc.connectionState === 'connected') {
        void this.startAgentSession(peerId, inboundPromise).catch((error: unknown) => {
          console.error(`Failed to start VoiceAgent for ${peerId}:`, error)
        })
      } else if (pc.connectionState === 'failed' || pc.connectionState === 'closed') {
        this.closeClient(peerId)
      }
    }

    controlChannel.onopen = () => {
      this.log(`[voice ${peerId}] control channel open`)
      session.unwireControl?.()
      const ctx = this.createSessionContext(peerId, agent)
      const onSpeakRequest = this.options.voiceHandler?.onSpeakRequest
      session.unwireControl = wireVoiceAgentToDataChannel(agent, controlChannel, {
        onSpeak: onSpeakRequest
          ? (text) => {
              void onSpeakRequest(ctx, text)
            }
          : undefined,
      })
    }

    const offer = await pc.createOffer()
    await pc.setLocalDescription(offer)
    await pc.gatheringComplete()
    this.signaling.sendOffer(peerId, pc.localDescription!.toJSON())
    session.offerSent = true
    this.log(`[voice ${peerId}] offer sent (audio + ${VOICE_CONTROL_CHANNEL_LABEL} DC)`)

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

    session.unwireSpeechForward?.()
    session.unwireSpeechForward = this.wireSpeechEvents(peerId, session)

    await session.agentOut.writeSample(createKickFrame(), PCM_KICK_DURATION_MS)
    this.log(`[voice ${peerId}] VoiceAgent started — mic → STT, TTS → browser`)
  }

  private createSessionContext(peerId: string, agent: VoiceAgent): VoiceSessionContext {
    return {
      peerId,
      agent,
      speak: (text: string) => agent.sendTextToTTS(text),
    }
  }

  /**
   * Forwards speech events to the browser and invokes {@link VoiceAgentSessionHostOptions.voiceHandler}.
   */
  private wireSpeechEvents(peerId: string, session: ClientSession): () => void {
    const voiceHandler = this.options.voiceHandler
    if (!voiceHandler?.onSpeechEvent) {
      return forwardVoiceAgentSpeechToDataChannel(session.agent, session.controlChannel)
    }

    const ctx = this.createSessionContext(peerId, session.agent)
    let active = true

    void (async () => {
      for await (const event of session.agent.speechEvents()) {
        if (!active) break
        try {
          await voiceHandler.onSpeechEvent!(ctx, event)
        } catch (error: unknown) {
          console.error(`[voice ${peerId}] voiceHandler.onSpeechEvent failed:`, error)
        }
        this.sendSpeechEventToControlChannel(session.controlChannel, event)
      }
    })()

    return () => {
      active = false
    }
  }

  private sendSpeechEventToControlChannel(
    channel: RTCDataChannel,
    event: SpeechEvent,
  ): void {
    if (channel.readyState !== 'open') return
    channel.send(JSON.stringify(speechEventToControlMessage(event)))
  }

  private async onAnswerReceived(peerId: string, sdp: RTCSessionDescriptionInit): Promise<void> {
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
      this.log(`[voice ${peerId}] answer applied, connectionState=${session.pc.connectionState}`)
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
    if (!session) {
      this.sessionBudget.release(peerId)
      return
    }
    session.unwireControl?.()
    session.unwireSpeechForward?.()
    void session.agent.stop().catch(() => undefined)
    session.pc.close()
    this.sessions.delete(peerId)
    this.sessionBudget.release(peerId)
    this.log(`[voice ${peerId}] VoiceAgent stopped, connection closed`)
  }
}
