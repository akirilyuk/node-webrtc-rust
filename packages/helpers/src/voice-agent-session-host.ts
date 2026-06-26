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
  VOICE_SYNC_CHANNEL_LABEL,
  agentSpeakToControlMessage,
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
import type {
  VoiceSessionContext,
  VoiceSessionHandler,
  DataChannelKind,
} from './voice-session-handler.js'

/** Debounce before tearing down a peer after ICE/PC disconnect (allows brief blips). */
const PEER_TRANSPORT_DISCONNECT_GRACE_MS = 5_000

export const VOICE_AGENT_SERVER_PEER_ID = 'voice-agent-server'

/** @deprecated Use {@link VOICE_AGENT_SERVER_PEER_ID}. */
export const SERVER_PEER_ID = VOICE_AGENT_SERVER_PEER_ID

interface IceServerConfig {
  urls: string | string[]
}

interface ClientSession {
  pc: RTCPeerConnection
  controlChannel: RTCDataChannel
  syncChannel?: RTCDataChannel
  agent?: VoiceAgent
  agentOut?: LocalAudioTrack
  inboundTrack?: RemoteAudioTrack
  agentStarted: boolean
  peerConnectedNotified: boolean
  peerSignalingJoined: boolean
  unwireControl?: () => void
  unwireSync?: () => void
  unwireSpeechForward?: () => void
  remoteDescriptionSet: boolean
  offerSent: boolean
  pendingAnswer: RTCSessionDescriptionInit | null
  pendingIce: RTCIceCandidateInit[]
  /** Cleared in {@link VoiceAgentSessionHost.closeClient}. */
  micTrackTimer?: ReturnType<typeof setTimeout>
  transportDisconnectTimer?: ReturnType<typeof setTimeout>
  resolveMicTrack?: (track: RemoteAudioTrack) => void
  rejectMicTrack?: (error: Error) => void
}

export interface VoiceAgentSessionHostOptions {
  voiceConfig: VoiceAgentConfig
  /** `data-only` skips audio tracks and VoiceAgent (DataChannels only). */
  sessionMode?: 'voice' | 'data-only'
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
   * Optional second outbound data channel for high-frequency binary sync.
   * Defaults to disabled; label {@link VOICE_SYNC_CHANNEL_LABEL} when enabled.
   */
  syncChannel?: {
    enabled?: boolean
    label?: string
    ordered?: boolean
  }
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
  private readonly sessionMode: 'voice' | 'data-only'
  /** Per-peer WebRTC reconnect attempts after `connectionState=failed`. */
  private readonly reconnectAttempts = new Map<string, number>()

  constructor(
    private readonly signaling: SignalingClient,
    private readonly iceServers: IceServerConfig[],
    private readonly options: VoiceAgentSessionHostOptions,
  ) {
    this.clientPeerIdPrefix = options.clientPeerIdPrefix ?? 'client-'
    this.log = options.log ?? ((message) => console.log(message))
    this.sessionBudget = options.sessionBudget ?? getProcessVoiceSessionBudget()
    this.sessionMode = options.sessionMode ?? 'voice'

    this.signaling.on('peer-joined', (peerId) => {
      if (peerId === VOICE_AGENT_SERVER_PEER_ID) return
      if (!peerId.startsWith(this.clientPeerIdPrefix)) return
      // Same tab id can re-join after refresh without a clean peer-left (stale VoiceAgent/PC).
      if (this.sessions.has(peerId)) {
        this.log(`[voice ${peerId}] peer re-joined — replacing stale session`)
        this.closeClient(peerId)
      }
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

  /** Disconnect one browser peer and release its session budget slot. */
  disconnectPeer(peerId: string): void {
    this.closeClient(peerId)
  }

  /** Current process session budget (active / max / rejected). */
  get sessionBudgetSnapshot(): VoiceSessionBudgetSnapshot {
    return this.sessionBudget.snapshot()
  }

  /**
   * Synthesize `text` on every connected client that has a running VoiceAgent.
   * Invokes {@link VoiceSessionHandler.onBroadcastSpeak} when set; otherwise TTS each agent.
   */
  async broadcastSpeak(text: string): Promise<string[]> {
    const trimmed = text.trim()
    if (!trimmed) return []

    const contexts: VoiceSessionContext[] = []
    for (const [peerId, session] of this.sessions) {
      if (this.sessionMode === 'voice' && !session.agentStarted) continue
      if (!session.agent) continue
      contexts.push(
        this.createSessionContext(
          peerId,
          session.agent,
          session.controlChannel,
          session.syncChannel,
        ),
      )
    }

    const onBroadcastSpeak = this.options.voiceHandler?.onBroadcastSpeak
    if (onBroadcastSpeak) {
      const spoken = await onBroadcastSpeak(trimmed, contexts)
      this.log(
        `[voice-server] broadcast via handler: "${trimmed.slice(0, 80)}" → ${spoken.join(', ')}`,
      )
      return spoken
    }

    const spoken: string[] = []
    await Promise.all(
      contexts.map(async (ctx) => {
        try {
          await ctx.speak(trimmed, { nonBlocking: true })
          spoken.push(ctx.peerId)
          this.log(`[voice ${ctx.peerId}] broadcast speak: "${trimmed.slice(0, 80)}"`)
        } catch (error: unknown) {
          console.error(`[voice ${ctx.peerId}] broadcast speak failed:`, error)
        }
      }),
    )
    return spoken
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
    const dataOnly = this.sessionMode === 'data-only'

    const controlChannel = pc.createDataChannel(VOICE_CONTROL_CHANNEL_LABEL, { ordered: true })
    const syncEnabled = this.options.syncChannel?.enabled ?? false
    const syncChannel = syncEnabled
      ? pc.createDataChannel(this.options.syncChannel?.label ?? VOICE_SYNC_CHANNEL_LABEL, {
          ordered: this.options.syncChannel?.ordered ?? false,
        })
      : undefined

    let agent: VoiceAgent | undefined
    let agentOut: LocalAudioTrack | undefined
    let inboundPromise: Promise<RemoteAudioTrack> | undefined

    if (dataOnly) {
      this.log(`[data ${peerId}] negotiating DataChannels only (no audio)`)
    } else {
      agentOut = new LocalAudioTrack(`agent-out-${peerId}`, 'voice-agent')
      agent = new VoiceAgent(this.options.voiceConfig)
      await pc.addTrack(agentOut)
    }

    const session: ClientSession = {
      pc,
      agentOut,
      controlChannel,
      syncChannel,
      agent,
      agentStarted: false,
      peerConnectedNotified: false,
      peerSignalingJoined: true,
      remoteDescriptionSet: false,
      offerSent: false,
      pendingAnswer: null,
      pendingIce: [],
    }
    this.sessions.set(peerId, session)

    if (!dataOnly && agent) {
      inboundPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
        session.resolveMicTrack = resolve
        session.rejectMicTrack = reject
      })
      void inboundPromise.catch((error: unknown) => {
        const message = error instanceof Error ? error.message : String(error)
        this.log(`[voice ${peerId}] ${message}`)
        this.closeClient(peerId)
      })

      pc.ontrack = (event) => {
        if (event.track.kind !== 'audio') return
        this.clearMicTrackTimer(session)
        session.resolveMicTrack?.(event.track as RemoteAudioTrack)
        session.resolveMicTrack = undefined
        session.rejectMicTrack = undefined
      }
    }

    pc.onicecandidate = (event) => {
      if (event.candidate) {
        this.signaling.sendIceCandidate(peerId, event.candidate.toJSON())
      }
    }

    pc.oniceconnectionstatechange = () => {
      const tag = dataOnly ? 'data' : 'voice'
      const iceState = pc.iceConnectionState
      this.log(`[${tag} ${peerId}] iceConnectionState=${iceState}`)
      if (iceState === 'connected' || iceState === 'completed') {
        this.clearTransportDisconnectTimer(session)
      } else if (iceState === 'disconnected') {
        this.scheduleTransportDisconnect(peerId, session)
      } else if (iceState === 'failed' || iceState === 'closed') {
        this.clearTransportDisconnectTimer(session)
        this.closeClient(peerId)
      }
    }

    pc.onconnectionstatechange = () => {
      const tag = dataOnly ? 'data' : 'voice'
      this.log(`[${tag} ${peerId}] connectionState=${pc.connectionState}`)
      if (pc.connectionState === 'connected') {
        this.clearTransportDisconnectTimer(session)
        this.reconnectAttempts.delete(peerId)
        if (dataOnly) {
          this.maybeNotifyPeerConnected(peerId, session)
        } else if (inboundPromise) {
          void this.startAgentSession(peerId, inboundPromise).catch((error: unknown) => {
            console.error(`Failed to start VoiceAgent for ${peerId}:`, error)
          })
        }
      } else if (pc.connectionState === 'disconnected') {
        this.scheduleTransportDisconnect(peerId, session)
      } else if (pc.connectionState === 'failed' || pc.connectionState === 'closed') {
        this.clearTransportDisconnectTimer(session)
        this.log(`[${tag} ${peerId}] connection ${pc.connectionState} — closing peer`)
        this.closeClient(peerId)
      }
    }

    controlChannel.onclose = () => {
      if (!this.sessions.has(peerId)) return
      const tag = dataOnly ? 'data' : 'voice'
      this.log(`[${tag} ${peerId}] control channel closed`)
      this.closeClient(peerId)
    }

    controlChannel.onopen = () => {
      const tag = dataOnly ? 'data' : 'voice'
      this.log(`[${tag} ${peerId}] control channel open`)
      session.unwireControl?.()
      const ctx = this.createSessionContext(peerId, session.agent, controlChannel, syncChannel)
      const voiceHandler = this.options.voiceHandler
      if (dataOnly) {
        const onDataChannelMessage = voiceHandler?.onDataChannelMessage
        const onDataChannelBinary = voiceHandler?.onDataChannelBinary
        const previousOnMessage = controlChannel.onmessage
        controlChannel.onmessage = (event) => {
          previousOnMessage?.(event)
          if (typeof event.data !== 'string') {
            if (!onDataChannelBinary) return
            const binary =
              event.data instanceof ArrayBuffer
                ? Buffer.from(event.data)
                : Buffer.isBuffer(event.data)
                  ? event.data
                  : Buffer.from(event.data as Uint8Array)
            void onDataChannelBinary(ctx, binary, 'control')
            return
          }
          if (onDataChannelMessage) {
            void onDataChannelMessage(ctx, event.data)
          }
        }
        session.unwireControl = () => {
          controlChannel.onmessage = previousOnMessage
        }
        this.maybeNotifyPeerConnected(peerId, session)
        return
      }

      if (!agent) return
      const onSpeakRequest = voiceHandler?.onSpeakRequest
      const onDataChannelMessage = voiceHandler?.onDataChannelMessage
      const onDataChannelBinary = voiceHandler?.onDataChannelBinary
      session.unwireControl = wireVoiceAgentToDataChannel(agent, controlChannel, {
        onSpeak: onSpeakRequest
          ? (text) => {
              void onSpeakRequest(ctx, text)
            }
          : undefined,
        onDataChannelMessage: onDataChannelMessage
          ? (payload) => {
              void onDataChannelMessage(ctx, payload)
            }
          : undefined,
        onDataChannelBinary: onDataChannelBinary
          ? (data) => {
              void onDataChannelBinary(ctx, data, 'control')
            }
          : undefined,
      })
      // VoiceAgent may finish startAgentSession before the control DC opens; the first
      // maybeNotifyPeerConnected then no-ops. Data-only mode already retries here.
      this.maybeNotifyPeerConnected(peerId, session)
    }

    if (syncChannel) {
      syncChannel.binaryType = 'arraybuffer'
      syncChannel.onopen = () => {
        const tag = dataOnly ? 'data' : 'voice'
        this.log(`[${tag} ${peerId}] sync channel open (${syncChannel.label})`)
      }
      session.unwireSync = this.wireSyncChannel(peerId, session, syncChannel)
    }

    const offer = await pc.createOffer()
    await pc.setLocalDescription(offer)
    await pc.gatheringComplete()
    this.signaling.sendOffer(peerId, pc.localDescription!.toJSON())
    session.offerSent = true
    const tag = dataOnly ? 'data' : 'voice'
    this.log(
      `[${tag} ${peerId}] offer sent (${dataOnly ? '' : 'audio + '}${VOICE_CONTROL_CHANNEL_LABEL} DC${syncChannel ? ` + ${syncChannel.label}` : ''})`,
    )

    if (session.pendingAnswer) {
      const sdp = session.pendingAnswer
      session.pendingAnswer = null
      await this.applyAnswer(peerId, sdp)
    }
  }

  private maybeNotifyPeerConnected(peerId: string, session: ClientSession): void {
    if (session.peerConnectedNotified) return
    if (session.pc.connectionState !== 'connected') return
    if (session.controlChannel.readyState !== 'open') return
    session.peerConnectedNotified = true
    const ctx = this.createSessionContext(
      peerId,
      session.agent,
      session.controlChannel,
      session.syncChannel,
    )
    void Promise.resolve(this.options.voiceHandler?.onPeerConnected?.(ctx)).catch(
      (error: unknown) => {
        console.error(`[session ${peerId}] voiceHandler.onPeerConnected failed:`, error)
      },
    )
  }

  private async startAgentSession(
    peerId: string,
    inboundPromise: Promise<RemoteAudioTrack>,
  ): Promise<void> {
    const session = this.sessions.get(peerId)
    if (!session || session.agentStarted || !session.agent || !session.agentOut) return
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
    this.maybeNotifyPeerConnected(peerId, session)
  }

  private createSessionContext(
    peerId: string,
    agent: VoiceAgent | undefined,
    controlChannel: RTCDataChannel,
    syncChannel?: RTCDataChannel,
  ): VoiceSessionContext {
    const sendBinary = (data: Buffer | Uint8Array, channel: DataChannelKind = 'sync') => {
      const target =
        channel === 'sync' && syncChannel?.readyState === 'open'
          ? syncChannel
          : controlChannel.readyState === 'open'
            ? controlChannel
            : null
      if (!target) return
      target.send(data)
    }
    return {
      peerId,
      roomId: this.signaling.room,
      agent,
      speak: (text: string, options?) => {
        if (!agent) return Promise.resolve()
        const trimmed = text.trim()
        if (trimmed.length > 0 && controlChannel.readyState === 'open') {
          controlChannel.send(
            JSON.stringify(
              agentSpeakToControlMessage(trimmed, { ts: new Date().toISOString() }),
            ),
          )
        }
        return agent.sendTextToTTS(text, options)
      },
      sendToClient: (payload: unknown) => {
        if (controlChannel.readyState !== 'open') return
        controlChannel.send(JSON.stringify(payload))
      },
      sendBinaryToClient: (data, channel) => sendBinary(data, channel),
    }
  }

  private wireSyncChannel(
    peerId: string,
    session: ClientSession,
    syncChannel: RTCDataChannel,
  ): () => void {
    const onDataChannelBinary = this.options.voiceHandler?.onDataChannelBinary
    if (!onDataChannelBinary) {
      return () => undefined
    }
    const ctx = this.createSessionContext(
      peerId,
      session.agent,
      session.controlChannel,
      syncChannel,
    )
    const previousOnMessage = syncChannel.onmessage
    syncChannel.onmessage = (event) => {
      previousOnMessage?.(event)
      if (typeof event.data === 'string') return
      const binary =
        event.data instanceof ArrayBuffer
          ? Buffer.from(event.data)
          : Buffer.isBuffer(event.data)
            ? event.data
            : Buffer.from(event.data as Uint8Array)
      void onDataChannelBinary(ctx, binary, 'sync')
    }
    return () => {
      syncChannel.onmessage = previousOnMessage
    }
  }

  /**
   * Forwards speech events to the browser and invokes {@link VoiceAgentSessionHostOptions.voiceHandler}.
   */
  private wireSpeechEvents(peerId: string, session: ClientSession): () => void {
    const voiceHandler = this.options.voiceHandler
    if (!session.agent) {
      return () => undefined
    }
    if (!voiceHandler?.onSpeechEvent) {
      return forwardVoiceAgentSpeechToDataChannel(session.agent, session.controlChannel)
    }

    if (!session.agent) {
      return () => undefined
    }
    const agent = session.agent

    const ctx = this.createSessionContext(
      peerId,
      agent,
      session.controlChannel,
      session.syncChannel,
    )
    let active = true

    void (async () => {
      for await (const event of agent.speechEvents()) {
        if (!active) break
        this.sendSpeechEventToControlChannel(session.controlChannel, event)
        void Promise.resolve(voiceHandler.onSpeechEvent!(ctx, event)).catch((error: unknown) => {
          console.error(`[voice ${peerId}] voiceHandler.onSpeechEvent failed:`, error)
        })
      }
    })()

    return () => {
      active = false
    }
  }

  private sendSpeechEventToControlChannel(channel: RTCDataChannel, event: SpeechEvent): void {
    if (channel.readyState !== 'open') return
    channel.send(
      JSON.stringify(speechEventToControlMessage(event, { ts: new Date().toISOString() })),
    )
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

  private clearMicTrackTimer(session: ClientSession): void {
    if (session.micTrackTimer !== undefined) {
      clearTimeout(session.micTrackTimer)
      session.micTrackTimer = undefined
    }
  }

  private startMicTrackTimer(peerId: string, session: ClientSession): void {
    this.clearMicTrackTimer(session)
    session.micTrackTimer = setTimeout(() => {
      session.rejectMicTrack?.(
        new Error(
          `timed out waiting for mic track from ${peerId} (check ICE — use http://127.0.0.1 and WEBRTC_NAT_1TO1_IPS=127.0.0.1 on the server)`,
        ),
      )
      session.rejectMicTrack = undefined
      session.resolveMicTrack = undefined
    }, 30_000)
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
      if (this.sessionMode !== 'data-only') {
        this.startMicTrackTimer(peerId, session)
      }
      const tag = this.sessionMode === 'data-only' ? 'data' : 'voice'
      this.log(`[${tag} ${peerId}] answer applied, connectionState=${session.pc.connectionState}`)
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

  private clearTransportDisconnectTimer(session: ClientSession): void {
    if (session.transportDisconnectTimer) {
      clearTimeout(session.transportDisconnectTimer)
      session.transportDisconnectTimer = undefined
    }
  }

  private scheduleTransportDisconnect(peerId: string, session: ClientSession): void {
    if (session.transportDisconnectTimer) return
    session.transportDisconnectTimer = setTimeout(() => {
      session.transportDisconnectTimer = undefined
      const current = this.sessions.get(peerId)
      if (!current || current !== session) return
      const pc = session.pc
      const iceState = pc.iceConnectionState
      const connState = pc.connectionState
      if (
        iceState === 'disconnected' ||
        iceState === 'failed' ||
        iceState === 'closed' ||
        connState === 'disconnected' ||
        connState === 'failed' ||
        connState === 'closed'
      ) {
        const tag = this.sessionMode === 'data-only' ? 'data' : 'voice'
        this.log(
          `[${tag} ${peerId}] transport still down after ${PEER_TRANSPORT_DISCONNECT_GRACE_MS}ms — closing peer`,
        )
        this.closeClient(peerId)
      }
    }, PEER_TRANSPORT_DISCONNECT_GRACE_MS)
  }

  private closeClient(peerId: string): void {
    const session = this.sessions.get(peerId)
    if (!session) {
      this.sessionBudget.release(peerId)
      return
    }
    if (session.peerConnectedNotified) {
      const ctx = this.createSessionContext(
        peerId,
        session.agent,
        session.controlChannel,
        session.syncChannel,
      )
      void Promise.resolve(this.options.voiceHandler?.onPeerDisconnected?.(ctx)).catch(
        (error: unknown) => {
          console.error(`[session ${peerId}] voiceHandler.onPeerDisconnected failed:`, error)
        },
      )
    } else if (session.peerSignalingJoined) {
      const ctx = this.createSessionContext(
        peerId,
        session.agent,
        session.controlChannel,
        session.syncChannel,
      )
      void Promise.resolve(this.options.voiceHandler?.onPeerSignalingLost?.(ctx)).catch(
        (error: unknown) => {
          console.error(`[session ${peerId}] voiceHandler.onPeerSignalingLost failed:`, error)
        },
      )
    }
    this.clearMicTrackTimer(session)
    this.clearTransportDisconnectTimer(session)
    session.resolveMicTrack = undefined
    session.rejectMicTrack = undefined
    session.unwireControl?.()
    session.unwireSync?.()
    session.unwireSpeechForward?.()
    if (session.agent) {
      void session.agent.stop().catch(() => undefined)
    }
    session.pc.close()
    this.sessions.delete(peerId)
    this.sessionBudget.release(peerId)
    const tag = this.sessionMode === 'data-only' ? 'data' : 'voice'
    this.log(`[${tag} ${peerId}] session stopped, connection closed`)
  }
}
