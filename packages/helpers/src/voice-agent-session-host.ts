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

import { createOfferGatherWithIceCredentials } from './offer-ice-gather.js'
import { createKickFrame, PCM_KICK_DURATION_MS } from './pcm.js'
import {
  getProcessVoiceSessionBudget,
  type VoiceSessionBudget,
  type VoiceSessionBudgetSnapshot,
  type VoiceSessionLease,
} from './voice-session-budget.js'
import { flushVoiceControlChannel } from './control-channel-flush.js'
import type {
  VoiceSessionContext,
  VoiceSessionHandler,
  DataChannelKind,
} from './voice-session-handler.js'

/** Debounce before tearing down a peer after ICE/PC disconnect (allows brief blips). */
const PEER_TRANSPORT_DISCONNECT_GRACE_MS = 5_000
/** Bound native peer cleanup so session budget release cannot hang forever. */
const PEER_NATIVE_CLOSE_TIMEOUT_MS = 5_000

type AwaitablePeerConnection = RTCPeerConnection & {
  closeAsync?: () => Promise<void>
}

/** Per-component teardown status for capacity-safe close. */
export type TeardownComponentStatus = 'ok' | 'timed_out' | 'failed' | 'absent'

/** Strict combined outcome of peer close + agent stop (capacity-safe teardown). */
export type PeerCloseOutcome =
  | { status: 'closed'; pc: 'ok' | 'absent'; agent: 'ok' | 'absent' }
  | {
      status: 'timed_out'
      quarantined: true
      pc: TeardownComponentStatus
      agent: TeardownComponentStatus
    }
  | {
      status: 'failed'
      quarantined: true
      pc: TeardownComponentStatus
      agent: TeardownComponentStatus
      error?: unknown
    }
  | { status: 'absent' }

type NativeCloseRaceResult = {
  status: 'ok' | 'timed_out' | 'failed'
  error?: unknown
  /**
   * When status is `timed_out`, settles later with the eventual native result so
   * quarantine can clear exactly once if both PC and agent are confirmed safe.
   */
  pending?: Promise<'ok' | 'failed'>
}

type AgentStopRaceResult = {
  status: 'ok' | 'timed_out' | 'failed' | 'absent'
  error?: unknown
  pending?: Promise<'ok' | 'failed'>
}

async function awaitPeerConnectionClosed(
  pc: AwaitablePeerConnection,
  timeoutMs: number,
): Promise<NativeCloseRaceResult> {
  if (typeof pc.closeAsync === 'function') {
    let timedOut = false
    let timer: ReturnType<typeof setTimeout> | undefined
    let settlePending!: (value: 'ok' | 'failed') => void
    const pending = new Promise<'ok' | 'failed'>((resolve) => {
      settlePending = resolve
    })
    let error: unknown
    const closeWork = (async () => {
      try {
        await pc.closeAsync!()
        settlePending('ok')
      } catch (err: unknown) {
        error = err
        settlePending('failed')
      }
    })()
    try {
      await Promise.race([
        closeWork,
        new Promise<void>((resolve) => {
          timer = setTimeout(() => {
            timedOut = true
            resolve()
          }, timeoutMs)
        }),
      ])
    } finally {
      if (timer) clearTimeout(timer)
    }
    if (timedOut) return { status: 'timed_out', pending }
    if (error !== undefined) return { status: 'failed', error }
    return { status: 'ok' }
  }
  try {
    pc.close()
  } catch (error: unknown) {
    return { status: 'failed', error }
  }
  return { status: 'ok' }
}

async function awaitAgentStopped(
  agent: { stop: () => Promise<void> } | undefined,
  timeoutMs: number,
): Promise<AgentStopRaceResult> {
  if (!agent) {
    return { status: 'absent' }
  }
  let timedOut = false
  let timer: ReturnType<typeof setTimeout> | undefined
  let settlePending!: (value: 'ok' | 'failed') => void
  const pending = new Promise<'ok' | 'failed'>((resolve) => {
    settlePending = resolve
  })
  let error: unknown
  const stopWork = (async () => {
    try {
      await agent.stop()
      settlePending('ok')
    } catch (err: unknown) {
      error = err
      settlePending('failed')
    }
  })()
  try {
    await Promise.race([
      stopWork,
      new Promise<void>((resolve) => {
        timer = setTimeout(() => {
          timedOut = true
          resolve()
        }, timeoutMs)
      }),
    ])
  } finally {
    if (timer) clearTimeout(timer)
  }
  if (timedOut) return { status: 'timed_out', pending }
  if (error !== undefined) return { status: 'failed', error }
  return { status: 'ok' }
}

function componentOk(status: TeardownComponentStatus): boolean {
  return status === 'ok' || status === 'absent'
}

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
  /** Opaque budget lease for this session (not peerId-keyed). */
  budgetLease: VoiceSessionLease
  /** True only after `VoiceAgent.attach()` + `VoiceAgent.start()` completed successfully. */
  agentStarted: boolean
  /** Guards concurrent `startAgentSession` attempts; cleared on failure so a retry can proceed. */
  agentStartInProgress: boolean
  peerTransportReadyNotified: boolean
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
  /** Held until PC connected + control DC open; consumed by {@link maybeStartAgentWhenTransportReady}. */
  inboundPromise?: Promise<RemoteAudioTrack>
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
  /** In-flight peer teardowns (counted for host close / idle teardown). */
  private readonly closingPeers = new Map<string, Promise<PeerCloseOutcome>>()
  /** In-flight connects (counted for host close / idle teardown). */
  private readonly connectingPeers = new Map<string, Promise<void>>()
  /**
   * Per-peer FIFO queue of connect/close work. Public signaling events enqueue;
   * queued ops call private `*Inner` methods directly (no nested enqueue / depth bypass).
   */
  private readonly peerOpTail = new Map<string, Promise<unknown>>()
  /** Host is shutting down — reject new connects. */
  private hostClosing = false
  /**
   * Leases held after PC close and/or agent stop timed out / failed.
   * Keeps budget occupied until both sides converge safely or process recycle.
   */
  private readonly quarantinedLeases = new Set<VoiceSessionLease>()
  /** True while any quarantine remains — host must not admit replacement work. */
  private recycleRequired = false
  /** Late PC/agent convergence waiters keyed by lease (cleared on release). */
  private readonly quarantineWaits = new Map<
    VoiceSessionLease,
    {
      peerId: string
      pc?: 'ok' | 'failed' | 'pending'
      agent?: 'ok' | 'failed' | 'pending'
    }
  >()

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
      this.log(
        `[voice ${peerId}] peer-joined — connectClient starting (activeClients=${this.activeClientCount}, mode=${this.sessionMode})`,
      )
      // Queued: close-during-connect runs after connect; reconnect-during-close waits then acquires a new lease.
      void this.enqueuePeerOp(peerId, async () => {
        if (this.hostClosing) {
          this.log(`[voice ${peerId}] peer-joined ignored — host is closing`)
          return
        }
        if (this.sessions.has(peerId)) {
          this.log(`[voice ${peerId}] peer re-joined — replacing stale session`)
          await this.closeClientInner(peerId)
        }
        try {
          await this.connectClientInner(peerId)
        } catch (error: unknown) {
          console.error(`Failed to connect client ${peerId}:`, error)
          await this.closeClientInner(peerId)
        }
      }).catch((error: unknown) => {
        console.error(`Failed to handle peer-joined for ${peerId}:`, error)
      })
    })

    this.signaling.on('answer', ({ peerId, sdp }) => {
      void this.onAnswerReceived(peerId, sdp)
    })

    this.signaling.on('ice-candidate', ({ peerId, candidate }) => {
      void this.addRemoteIce(peerId, candidate)
    })

    this.signaling.on('peer-left', (peerId) => {
      void this.enqueuePeerOp(peerId, () => this.closeClientInner(peerId)).catch(
        (error: unknown) => {
          console.error(`Failed to close client ${peerId} after peer-left:`, error)
        },
      )
    })
  }

  /**
   * Active + connecting + closing browser peers (for idle teardown / capacity).
   * Closing peers stay counted so SessionPod does not tear down early.
   */
  get activeClientCount(): number {
    const ids = new Set<string>([
      ...this.sessions.keys(),
      ...this.closingPeers.keys(),
      ...this.connectingPeers.keys(),
    ])
    return ids.size
  }

  /**
   * Disconnect one browser peer. Releases the session budget slot only after
   * native peer cleanup confirms closed. Timed-out / failed native close
   * quarantines the lease (capacity stays occupied; host becomes non-assignable).
   */
  async disconnectPeer(peerId: string): Promise<PeerCloseOutcome> {
    return this.enqueuePeerOp(peerId, () => this.closeClientInner(peerId))
  }

  /** Current process session budget plus host quarantine / recycle signal. */
  get sessionBudgetSnapshot(): VoiceSessionBudgetSnapshot {
    const snap = this.sessionBudget.snapshot()
    return {
      ...snap,
      quarantined: this.quarantinedLeases.size,
      recycleRequired: this.recycleRequired,
    }
  }

  /** Native-close quarantines that still occupy capacity. */
  get quarantinedCount(): number {
    return this.quarantinedLeases.size
  }

  /**
   * True when this host must not accept new peers — orchestrator / SessionPod
   * should treat the runner as non-assignable until recycle.
   */
  get isRecycleRequired(): boolean {
    return this.recycleRequired
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
    this.hostClosing = true
    const connecting = [...this.connectingPeers.values()]
    if (connecting.length > 0) {
      await Promise.allSettled(connecting)
    }
    const peerIds = new Set<string>([
      ...this.sessions.keys(),
      ...this.closingPeers.keys(),
      ...this.connectingPeers.keys(),
    ])
    await Promise.all(
      [...peerIds].map((peerId) =>
        this.enqueuePeerOp(peerId, () => this.closeClientInner(peerId)).catch((error: unknown) => {
          console.error(`[voice ${peerId}] host close peer teardown failed:`, error)
        }),
      ),
    )
    const stillClosing = [...this.closingPeers.values()]
    if (stillClosing.length > 0) {
      await Promise.allSettled(stillClosing)
    }
  }

  /**
   * FIFO per-peer serializer. Unrelated signaling events wait their turn.
   * Callers must invoke private `*Inner` methods from `op` — never re-enter
   * {@link enqueuePeerOp} for the same peer (avoids self-deadlock without depth bypass).
   */
  private enqueuePeerOp<T>(peerId: string, op: () => Promise<T>): Promise<T> {
    const prev = this.peerOpTail.get(peerId) ?? Promise.resolve()
    const run = prev.catch(() => undefined).then(op)
    const tail = run.then(
      () => undefined,
      () => undefined,
    )
    this.peerOpTail.set(peerId, tail)
    void tail.finally(() => {
      if (this.peerOpTail.get(peerId) === tail) {
        this.peerOpTail.delete(peerId)
      }
    })
    return run
  }

  private async connectClientInner(peerId: string): Promise<void> {
    if (this.hostClosing) {
      this.log(`[voice ${peerId}] connect skipped — host is closing`)
      return
    }
    if (this.recycleRequired) {
      this.log(
        `[voice ${peerId}] connect skipped — host recycle required (quarantined=${this.quarantinedLeases.size})`,
      )
      return
    }

    const budgetLease = this.sessionBudget.tryAcquire(peerId)
    if (budgetLease == null) {
      const snap = this.sessionBudget.snapshot()
      console.error(
        '[voice-agent-session-host] session budget reject — peer was routed to this runner but process cap is full; check orchestrator assignment',
        { peerId, budget: snap },
      )
      this.log(
        `[voice ${peerId}] rejected — session budget full (${snap.active}/${snap.max}, rejectedTotal=${snap.rejectedTotal})`,
      )
      return
    }

    let resolveConnecting!: () => void
    const connectingFlight = new Promise<void>((resolve) => {
      resolveConnecting = resolve
    })
    this.connectingPeers.set(peerId, connectingFlight)

    const partial: {
      budgetLease: VoiceSessionLease
      pc?: RTCPeerConnection
      agent?: VoiceAgent
      registered: boolean
    } = { budgetLease, registered: false }
    try {
      await this.connectClientBuildSession(peerId, budgetLease, {
        onPeerCreated: (pc) => {
          partial.pc = pc
        },
        onAgentCreated: (agent) => {
          partial.agent = agent
        },
        onRegistered: (built) => {
          partial.pc = built.pc
          partial.agent = built.agent
          partial.registered = true
        },
      })
    } catch (error: unknown) {
      await this.teardownPartialConnect(peerId, partial)
      throw error
    } finally {
      resolveConnecting()
      this.connectingPeers.delete(peerId)
    }
  }

  private async teardownPartialConnect(
    peerId: string,
    partial: {
      budgetLease: VoiceSessionLease
      pc?: RTCPeerConnection
      agent?: VoiceAgent
      registered: boolean
    },
  ): Promise<void> {
    if (partial.registered) {
      // Registered sessions own the lease until closeClientInner releases it.
      await this.closeClientInner(peerId)
      return
    }
    this.reconnectAttempts.delete(peerId)
    const [closeResult, agentResult] = await Promise.all([
      partial.pc
        ? awaitPeerConnectionClosed(partial.pc, PEER_NATIVE_CLOSE_TIMEOUT_MS)
        : Promise.resolve({ status: 'ok' as const } satisfies NativeCloseRaceResult),
      awaitAgentStopped(partial.agent, PEER_NATIVE_CLOSE_TIMEOUT_MS),
    ])
    const outcome = this.finalizeTeardownCapacity(
      peerId,
      partial.budgetLease,
      closeResult,
      agentResult,
      'partial-connect',
      true,
    )
    if (outcome.status === 'failed' || outcome.status === 'timed_out') {
      this.log(
        `[voice ${peerId}] partial connect teardown quarantined (pc=${outcome.pc}, agent=${outcome.agent})`,
      )
    }
  }

  private async connectClientBuildSession(
    peerId: string,
    budgetLease: VoiceSessionLease,
    hooks: {
      onPeerCreated: (pc: RTCPeerConnection) => void
      onAgentCreated: (agent: VoiceAgent) => void
      onRegistered: (built: { pc: RTCPeerConnection; agent?: VoiceAgent }) => void
    },
  ): Promise<void> {
    const pc = new RTCPeerConnection({ iceServers: this.iceServers })
    hooks.onPeerCreated(pc)
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
      hooks.onAgentCreated(agent)
      await pc.addTrack(agentOut)
    }

    const session: ClientSession = {
      pc,
      agentOut,
      controlChannel,
      syncChannel,
      agent,
      budgetLease,
      agentStarted: false,
      agentStartInProgress: false,
      peerTransportReadyNotified: false,
      peerConnectedNotified: false,
      peerSignalingJoined: true,
      remoteDescriptionSet: false,
      offerSent: false,
      pendingAnswer: null,
      pendingIce: [],
    }
    this.sessions.set(peerId, session)
    hooks.onRegistered({ pc, agent })

    if (!dataOnly && agent) {
      inboundPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
        session.resolveMicTrack = resolve
        session.rejectMicTrack = reject
      })
      void inboundPromise.catch((error: unknown) => {
        const message = error instanceof Error ? error.message : String(error)
        this.log(`[voice ${peerId}] ${message}`)
        this.voidCloseClient(peerId)
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
        this.voidCloseClient(peerId)
      }
    }

    pc.onconnectionstatechange = () => {
      const tag = dataOnly ? 'data' : 'voice'
      this.log(`[${tag} ${peerId}] connectionState=${pc.connectionState}`)
      if (pc.connectionState === 'connected') {
        this.clearTransportDisconnectTimer(session)
        this.reconnectAttempts.delete(peerId)
        this.maybeNotifyPeerLifecycle(peerId, session)
        if (!dataOnly && inboundPromise) {
          session.inboundPromise = inboundPromise
          this.maybeStartAgentWhenTransportReady(peerId, session)
        }
      } else if (pc.connectionState === 'disconnected') {
        this.scheduleTransportDisconnect(peerId, session)
      } else if (pc.connectionState === 'failed' || pc.connectionState === 'closed') {
        this.clearTransportDisconnectTimer(session)
        this.log(`[${tag} ${peerId}] connection ${pc.connectionState} — closing peer`)
        this.voidCloseClient(peerId)
      }
    }

    controlChannel.onclose = () => {
      if (!this.sessions.has(peerId)) return
      const tag = dataOnly ? 'data' : 'voice'
      this.log(`[${tag} ${peerId}] control channel closed`)
      this.voidCloseClient(peerId)
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
        this.maybeNotifyPeerLifecycle(peerId, session)
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
      // Transport or agent start may complete before the control DC opens; retry here.
      this.maybeNotifyPeerLifecycle(peerId, session)
      this.maybeStartAgentWhenTransportReady(peerId, session)
    }

    if (syncChannel) {
      syncChannel.binaryType = 'arraybuffer'
      syncChannel.onopen = () => {
        const tag = dataOnly ? 'data' : 'voice'
        this.log(`[${tag} ${peerId}] sync channel open (${syncChannel.label})`)
      }
      session.unwireSync = this.wireSyncChannel(peerId, session, syncChannel)
    }

    const tag = dataOnly ? 'data' : 'voice'
    await createOfferGatherWithIceCredentials(pc, {
      onRetry: (attempt, sdpLen) => {
        this.log(
          `[${tag} ${peerId}] offer SDP missing ICE credentials (sdp_len=${sdpLen}), retry ${attempt}/2 with iceRestart`,
        )
      },
    })
    const localInit = pc.localDescription!.toJSON()
    this.signaling.sendOffer(peerId, localInit)
    session.offerSent = true
    const sdpLen = typeof localInit.sdp === 'string' ? localInit.sdp.length : 0
    this.log(
      `[${tag} ${peerId}] offer sent sdp_len=${sdpLen} (${dataOnly ? '' : 'audio + '}${VOICE_CONTROL_CHANNEL_LABEL} DC${syncChannel ? ` + ${syncChannel.label}` : ''})`,
    )

    if (session.pendingAnswer) {
      const sdp = session.pendingAnswer
      session.pendingAnswer = null
      await this.applyAnswer(peerId, sdp)
    }
  }

  /**
   * Idempotent readiness transitions: transport-ready (PC + control open), then
   * connected (voice: `agentStarted`; data-only: transport-ready alone).
   * Safe to call from PC state changes, control `onopen`, and agent start completion.
   */
  private maybeNotifyPeerLifecycle(peerId: string, session: ClientSession): void {
    if (session.pc.connectionState !== 'connected') return
    if (session.controlChannel.readyState !== 'open') return

    const ctx = this.createSessionContext(
      peerId,
      session.agent,
      session.controlChannel,
      session.syncChannel,
    )
    const voiceHandler = this.options.voiceHandler

    if (!session.peerTransportReadyNotified) {
      session.peerTransportReadyNotified = true
      try {
        void Promise.resolve(voiceHandler?.onPeerTransportReady?.(ctx)).catch((error: unknown) => {
          console.error(`[session ${peerId}] voiceHandler.onPeerTransportReady failed:`, error)
        })
      } catch (error: unknown) {
        console.error(`[session ${peerId}] voiceHandler.onPeerTransportReady failed:`, error)
      }
    }

    const agentReady = this.sessionMode === 'data-only' || session.agentStarted
    if (session.peerConnectedNotified || !agentReady) return

    session.peerConnectedNotified = true
    try {
      void Promise.resolve(voiceHandler?.onPeerConnected?.(ctx)).catch((error: unknown) => {
        console.error(`[session ${peerId}] voiceHandler.onPeerConnected failed:`, error)
      })
    } catch (error: unknown) {
      console.error(`[session ${peerId}] voiceHandler.onPeerConnected failed:`, error)
    }
  }

  /**
   * Starts VoiceAgent only when SCTP transport and control DataChannel are both ready.
   * Defers native agent work until the control DC is open so SCTP is not starved under load.
   */
  private maybeStartAgentWhenTransportReady(peerId: string, session: ClientSession): void {
    if (this.sessionMode === 'data-only') return
    if (
      session.agentStarted ||
      session.agentStartInProgress ||
      !session.agent ||
      !session.inboundPromise
    ) {
      return
    }
    if (session.pc.connectionState !== 'connected') return
    if (session.controlChannel.readyState !== 'open') return

    const inboundPromise = session.inboundPromise
    delete session.inboundPromise
    void this.startAgentSession(peerId, inboundPromise).catch((error: unknown) => {
      console.error(`Failed to start VoiceAgent for ${peerId}:`, error)
    })
  }

  private async startAgentSession(
    peerId: string,
    inboundPromise: Promise<RemoteAudioTrack>,
  ): Promise<void> {
    const session = this.sessions.get(peerId)
    if (
      !session ||
      session.agentStarted ||
      session.agentStartInProgress ||
      !session.agent ||
      !session.agentOut
    ) {
      return
    }
    session.agentStartInProgress = true

    let agentRunning = false
    try {
      session.inboundTrack = await inboundPromise
      const live = this.sessions.get(peerId)
      if (!live || live !== session || !session.agent || !session.agentOut) return

      await session.agent.attach({
        inboundTrack: session.inboundTrack,
        outboundTrack: session.agentOut,
      })
      await session.agent.start()
      agentRunning = true

      if (this.sessions.get(peerId) !== session) {
        await session.agent.stop().catch(() => undefined)
        return
      }

      session.unwireSpeechForward?.()
      session.unwireSpeechForward = this.wireSpeechEvents(peerId, session)

      await session.agentOut.writeSample(createKickFrame(), PCM_KICK_DURATION_MS)
      session.agentStarted = true
      this.log(`[voice ${peerId}] VoiceAgent started — mic → STT, TTS → browser`)
      this.maybeNotifyPeerLifecycle(peerId, session)
    } catch (error) {
      if (agentRunning && !session.agentStarted) {
        session.unwireSpeechForward?.()
        session.unwireSpeechForward = undefined
        await session.agent?.stop().catch(() => undefined)
      }
      throw error
    } finally {
      session.agentStartInProgress = false
    }
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
            JSON.stringify(agentSpeakToControlMessage(trimmed, { ts: new Date().toISOString() })),
          )
        }
        return agent.sendTextToTTS(text, options)
      },
      sendToClient: (payload: unknown) => {
        if (controlChannel.readyState !== 'open') return
        controlChannel.send(JSON.stringify(payload))
      },
      flushToClient: () => flushVoiceControlChannel(controlChannel),
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
      this.voidCloseClient(peerId)
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
        this.voidCloseClient(peerId)
      }
    }, PEER_TRANSPORT_DISCONNECT_GRACE_MS)
  }

  /** Fire-and-forget close queued on the per-peer serializer. */
  private voidCloseClient(peerId: string): void {
    void this.enqueuePeerOp(peerId, () => this.closeClientInner(peerId)).catch((error: unknown) => {
      console.error(`[voice ${peerId}] closeClient failed:`, error)
    })
  }

  /**
   * Tear down one peer. Must be invoked from {@link enqueuePeerOp} (or already-queued work).
   * Budget release happens after agent+peer teardown (or bounded timeout) in finally.
   * Lifecycle hooks are nonblocking so they cannot stall cleanup.
   */
  private async closeClientInner(peerId: string): Promise<PeerCloseOutcome> {
    const inFlight = this.closingPeers.get(peerId)
    if (inFlight) {
      return inFlight
    }

    const flight = this.closeClientTeardown(peerId).finally(() => {
      this.closingPeers.delete(peerId)
    })
    this.closingPeers.set(peerId, flight)
    return flight
  }

  private quarantineLease(lease: VoiceSessionLease, peerId: string, reason: string): void {
    if (this.quarantinedLeases.has(lease)) return
    this.quarantinedLeases.add(lease)
    this.recycleRequired = true
    this.log(
      `[voice ${peerId}] quarantined lease after ${reason} (quarantined=${this.quarantinedLeases.size}) — capacity held; recycle required`,
    )
  }

  private releaseQuarantinedLease(lease: VoiceSessionLease, peerId: string): void {
    if (!this.quarantinedLeases.delete(lease)) return
    this.quarantineWaits.delete(lease)
    // Budget.release is token-idempotent — safe if already released.
    this.sessionBudget.release(lease)
    if (this.quarantinedLeases.size === 0) {
      this.recycleRequired = false
      this.log(`[voice ${peerId}] quarantine cleared — host assignable again`)
    } else {
      this.log(
        `[voice ${peerId}] released one quarantine; ${this.quarantinedLeases.size} remain — recycle still required`,
      )
    }
  }

  /**
   * Release capacity only when both PC close and agent stop are confirmed.
   * Otherwise quarantine and optionally wait for late dual convergence.
   * Peers that never reached transport-ready skip quarantine on timeout/failure.
   */
  private finalizeTeardownCapacity(
    peerId: string,
    lease: VoiceSessionLease,
    closeResult: NativeCloseRaceResult,
    agentResult: AgentStopRaceResult,
    tag: string,
    hadLiveSession: boolean,
  ): PeerCloseOutcome {
    const pcStatus: TeardownComponentStatus = closeResult.status
    const agentStatus: TeardownComponentStatus = agentResult.status
    const bothOk = componentOk(pcStatus) && componentOk(agentStatus)

    if (bothOk) {
      this.sessionBudget.release(lease)
      this.quarantinedLeases.delete(lease)
      this.quarantineWaits.delete(lease)
      if (this.quarantinedLeases.size === 0) {
        this.recycleRequired = false
      }
      this.log(
        `[${tag} ${peerId}] teardown confirmed (pc=${pcStatus}, agent=${agentStatus}) — capacity released`,
      )
      return {
        status: 'closed',
        // PC close race never yields `absent`; agent stop may.
        pc: 'ok',
        agent: agentStatus === 'absent' ? 'absent' : 'ok',
      }
    }

    const reasonParts: string[] = []
    if (!componentOk(pcStatus)) reasonParts.push(`pc=${pcStatus}`)
    if (!componentOk(agentStatus)) reasonParts.push(`agent=${agentStatus}`)
    const reason = reasonParts.join(', ')

    if (!hadLiveSession) {
      this.sessionBudget.release(lease)
      this.quarantinedLeases.delete(lease)
      this.quarantineWaits.delete(lease)
      this.log(
        `[${tag} ${peerId}] pre-transport teardown incomplete (${reason}) — capacity released without quarantine`,
      )
      const error = agentResult.error ?? closeResult.error
      if (pcStatus === 'failed' || agentStatus === 'failed') {
        return {
          status: 'failed',
          pc: pcStatus,
          agent: agentStatus,
          ...(error !== undefined ? { error } : {}),
        }
      }
      return {
        status: 'timed_out',
        pc: pcStatus,
        agent: agentStatus,
      }
    }

    this.quarantineLease(lease, peerId, reason)

    const wait = {
      peerId,
      pc: (pcStatus === 'timed_out' ? 'pending' : componentOk(pcStatus) ? 'ok' : 'failed') as
        | 'ok'
        | 'failed'
        | 'pending',
      agent: (agentStatus === 'timed_out'
        ? 'pending'
        : componentOk(agentStatus)
          ? 'ok'
          : 'failed') as 'ok' | 'failed' | 'pending',
    }
    this.quarantineWaits.set(lease, wait)

    if (closeResult.pending) {
      void closeResult.pending
        .then((result) => this.onLateTeardownComponent(lease, 'pc', result))
        .catch((error: unknown) => {
          console.error(`[voice ${peerId}] late PC close observer failed:`, error)
          this.onLateTeardownComponent(lease, 'pc', 'failed')
        })
    }
    if (agentResult.pending) {
      void agentResult.pending
        .then((result) => this.onLateTeardownComponent(lease, 'agent', result))
        .catch((error: unknown) => {
          console.error(`[voice ${peerId}] late agent stop observer failed:`, error)
          this.onLateTeardownComponent(lease, 'agent', 'failed')
        })
    }

    const error = agentResult.error ?? closeResult.error
    if (pcStatus === 'failed' || agentStatus === 'failed') {
      return {
        status: 'failed',
        quarantined: true,
        pc: pcStatus,
        agent: agentStatus,
        ...(error !== undefined ? { error } : {}),
      }
    }
    return {
      status: 'timed_out',
      quarantined: true,
      pc: pcStatus,
      agent: agentStatus,
    }
  }

  private onLateTeardownComponent(
    lease: VoiceSessionLease,
    side: 'pc' | 'agent',
    result: 'ok' | 'failed',
  ): void {
    const wait = this.quarantineWaits.get(lease)
    if (!wait || !this.quarantinedLeases.has(lease)) return
    wait[side] = result
    if (result === 'failed') {
      this.log(
        `[voice ${wait.peerId}] late ${side} cleanup failed while quarantined — recycle remains required`,
      )
      return
    }
    if (wait.pc === 'ok' && wait.agent === 'ok') {
      this.releaseQuarantinedLease(lease, wait.peerId)
    }
  }

  private async closeClientTeardown(peerId: string): Promise<PeerCloseOutcome> {
    const session = this.sessions.get(peerId)
    if (!session) {
      return { status: 'absent' }
    }
    const budgetLease = session.budgetLease

    // Remove from the live map before teardown so reconnect can replace;
    // peer stays counted via closingPeers until this flight finishes.
    this.sessions.delete(peerId)
    this.reconnectAttempts.delete(peerId)

    if (session.peerTransportReadyNotified) {
      const ctx = this.createSessionContext(
        peerId,
        session.agent,
        session.controlChannel,
        session.syncChannel,
      )
      void Promise.resolve()
        .then(() => this.options.voiceHandler?.onPeerDisconnected?.(ctx))
        .catch((error: unknown) => {
          console.error(`[session ${peerId}] voiceHandler.onPeerDisconnected failed:`, error)
        })
    } else if (session.peerSignalingJoined) {
      const ctx = this.createSessionContext(
        peerId,
        session.agent,
        session.controlChannel,
        session.syncChannel,
      )
      void Promise.resolve()
        .then(() => this.options.voiceHandler?.onPeerSignalingLost?.(ctx))
        .catch((error: unknown) => {
          console.error(`[session ${peerId}] voiceHandler.onPeerSignalingLost failed:`, error)
        })
    }

    this.clearMicTrackTimer(session)
    this.clearTransportDisconnectTimer(session)
    session.resolveMicTrack = undefined
    session.rejectMicTrack = undefined
    delete session.inboundPromise
    try {
      session.unwireControl?.()
    } catch (error: unknown) {
      console.error(`[session ${peerId}] unwireControl failed:`, error)
    }
    try {
      session.unwireSync?.()
    } catch (error: unknown) {
      console.error(`[session ${peerId}] unwireSync failed:`, error)
    }
    try {
      session.unwireSpeechForward?.()
    } catch (error: unknown) {
      console.error(`[session ${peerId}] unwireSpeechForward failed:`, error)
    }

    const tag = this.sessionMode === 'data-only' ? 'data' : 'voice'
    const agent = session.agent
    session.agent = undefined

    // Bound PC close and agent stop independently; release only when both confirm.
    const [closeResult, agentResult] = await Promise.all([
      awaitPeerConnectionClosed(session.pc, PEER_NATIVE_CLOSE_TIMEOUT_MS),
      awaitAgentStopped(agent, PEER_NATIVE_CLOSE_TIMEOUT_MS),
    ])
    if (agentResult.status === 'failed' && agentResult.error !== undefined) {
      console.error(`[${tag} ${peerId}] VoiceAgent.stop failed:`, agentResult.error)
    }
    if (closeResult.status === 'failed' && closeResult.error !== undefined) {
      console.error(`[${tag} ${peerId}] native peer close failed:`, closeResult.error)
    }

    return this.finalizeTeardownCapacity(
      peerId,
      budgetLease,
      closeResult,
      agentResult,
      tag,
      session.peerTransportReadyNotified,
    )
  }
}
