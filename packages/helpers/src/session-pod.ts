/**
 * Multi-session voice pod — one Node process, many concurrent calls.
 *
 * - Single {@link SignalingServer} entry point on the pod
 * - Each sessionId = one signaling room + {@link VoiceAgentSessionHost}
 * - One VoiceAgent per WebRTC connection (no routing inside the agent)
 * - Idle session teardown when the last client disconnects
 */

import type { SignalingServer } from '@node-webrtc-rust/signaling'
import { SignalingClient } from '@node-webrtc-rust/signaling'
import type { VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

import {
  getProcessVoiceSessionBudget,
  type VoiceSessionBudget,
  type VoiceSessionBudgetSnapshot,
} from './voice-session-budget.js'
import {
  VOICE_AGENT_SERVER_PEER_ID,
  VoiceAgentSessionHost,
  type VoiceAgentSessionHostOptions,
} from './voice-agent-session-host.js'
import type { VoiceSessionHandler } from './voice-session-handler.js'
import { SessionPodCapacityFullError } from './session-pod-errors.js'

interface IceServerConfig {
  urls: string | string[]
}

export interface SessionPodOptions {
  /** WebSocket URL the pod uses to join rooms as the server-side peer. */
  signalingUrl: string
  iceServers: IceServerConfig[]
  voiceConfig: VoiceAgentConfig
  /** When true (default), tear down the session slot once the last client disconnects. */
  teardownIdleSessions?: boolean
  /** Called after a session slot is created or destroyed (metrics, orchestrator hooks). */
  onSessionChange?: (event: SessionPodChangeEvent) => void
  /** Hold the runner slot after the last client leaves so same-session reconnect can succeed. */
  rejoinGraceMs?: number
  /** Server-side signaling peer id (default {@link VOICE_AGENT_SERVER_PEER_ID}). */
  serverPeerId?: string
  /** Shared across all rooms in this pod (default: process env budget). */
  sessionBudget?: VoiceSessionBudget
  /**
   * Max prepared session slots (orchestrator `POST /api/sessions`).
   * `0` means unlimited. Should match `VOICE_MAX_CONCURRENT_SESSIONS` on runners.
   */
  maxPreparedSessions?: number
  /** Per-tab STT/TTS logic (same handler instance for every room in this pod). */
  voiceHandler?: VoiceSessionHandler
  /** Optional binary sync data channel per WebRTC connection. */
  syncChannel?: VoiceAgentSessionHostOptions['syncChannel']
  /** Passed to each room's {@link VoiceAgentSessionHost}. */
  sessionMode?: 'voice' | 'data-only'
  log?: (message: string) => void
}

export interface SessionPodChangeEvent {
  sessionId: string
  action: 'created' | 'destroyed'
  activeSessions: number
  /** Set when action is `destroyed` and the teardown was reason-coded. */
  endReason?: string
}

export interface SessionPodSessionInfo {
  sessionId: string
  connections: number
}

/** Default grace before tearing down an empty slot — same-session reconnect window. */
export const DEFAULT_SESSION_REJOIN_GRACE_MS = 5_000

interface SessionSlot {
  sessionId: string
  signaling: SignalingClient
  host: VoiceAgentSessionHost
  pendingEndReason?: string
  /** When false, a signaling `disconnected` event does not auto-rejoin (teardown). */
  reconnectEnabled: boolean
  reconnectInFlight: boolean
}

/**
 * Manages many independent voice sessions inside one Node process.
 */
export class SessionPod {
  private readonly slots = new Map<string, SessionSlot>()
  private readonly teardownTimers = new Map<string, ReturnType<typeof setTimeout>>()
  private readonly teardownIdle: boolean
  private readonly rejoinGraceMs: number
  private readonly log: (message: string) => void
  private readonly sessionBudget: VoiceSessionBudget

  constructor(
    private readonly signalingServer: SignalingServer,
    private readonly options: SessionPodOptions,
  ) {
    this.teardownIdle = options.teardownIdleSessions ?? true
    this.rejoinGraceMs = options.rejoinGraceMs ?? DEFAULT_SESSION_REJOIN_GRACE_MS
    this.log = options.log ?? ((message) => console.log(message))
    this.sessionBudget = options.sessionBudget ?? getProcessVoiceSessionBudget()
  }

  private cancelTeardownTimer(sessionId: string): void {
    const timer = this.teardownTimers.get(sessionId)
    if (timer) {
      clearTimeout(timer)
      this.teardownTimers.delete(sessionId)
    }
  }

  private scheduleIdleTeardown(sessionId: string, endReason?: string): void {
    if (!this.teardownIdle) return
    const slot = this.slots.get(sessionId)
    if (slot && endReason) {
      slot.pendingEndReason = endReason
    }
    // WebRTC close and signaling peer-left both notify SessionPod — arm grace once.
    if (this.teardownTimers.has(sessionId)) {
      return
    }
    if (this.rejoinGraceMs <= 0) {
      void this.teardownSession(sessionId, endReason).catch((error: unknown) => {
        console.error(`Failed to teardown idle session ${sessionId}:`, error)
      })
      return
    }
    const timer = setTimeout(() => {
      this.teardownTimers.delete(sessionId)
      const current = this.slots.get(sessionId)
      if (!current || current.host.activeClientCount > 0) return
      void this.teardownSession(
        sessionId,
        current.pendingEndReason ?? endReason,
      ).catch((error: unknown) => {
        console.error(`Failed to teardown idle session ${sessionId}:`, error)
      })
    }, this.rejoinGraceMs)
    this.teardownTimers.set(sessionId, timer)
    this.log(
      `[pod] session ${sessionId} idle — teardown in ${this.rejoinGraceMs}ms unless client rejoins`,
    )
  }

  get sessionBudgetSnapshot(): VoiceSessionBudgetSnapshot {
    return this.sessionBudget.snapshot()
  }

  get activeSessionCount(): number {
    return this.slots.size
  }

  get activeConnectionCount(): number {
    let total = 0
    for (const slot of this.slots.values()) {
      total += slot.host.activeClientCount
    }
    return total
  }

  listSessions(): SessionPodSessionInfo[] {
    return [...this.slots.values()].map((slot) => ({
      sessionId: slot.sessionId,
      connections: slot.host.activeClientCount,
    }))
  }

  async ensureSession(sessionId: string): Promise<void> {
    if (this.slots.has(sessionId)) return

    const maxPrepared = this.options.maxPreparedSessions ?? 0
    if (maxPrepared > 0 && this.slots.size >= maxPrepared) {
      this.log(
        `[pod] session prepare rejected — slot capacity full (${this.slots.size}/${maxPrepared}) sessionId=${sessionId}; orchestrator should not assign here`,
      )
      throw new SessionPodCapacityFullError(this.slots.size, maxPrepared)
    }

    const serverPeerId = this.options.serverPeerId ?? VOICE_AGENT_SERVER_PEER_ID
    const signaling = new SignalingClient({
      url: this.options.signalingUrl,
      room: sessionId,
      peerId: serverPeerId,
    })
    await signaling.connect()

    const voiceHandler = this.wrapVoiceHandler(sessionId, this.options.voiceHandler)
    const host = new VoiceAgentSessionHost(signaling, this.options.iceServers, {
      voiceConfig: this.options.voiceConfig,
      sessionMode: this.options.sessionMode,
      sessionBudget: this.sessionBudget,
      voiceHandler,
      syncChannel: this.options.syncChannel,
      log: this.options.log,
    })

    const slot: SessionSlot = {
      sessionId,
      signaling,
      host,
      reconnectEnabled: true,
      reconnectInFlight: false,
    }

    if (this.teardownIdle) {
      signaling.on('peer-joined', (peerId) => {
        if (!peerId.startsWith('client-')) return
        this.cancelTeardownTimer(sessionId)
      })
    }

    this.bindAgentSignalingReconnect(slot)
    this.slots.set(sessionId, slot)
    this.options.onSessionChange?.({
      sessionId,
      action: 'created',
      activeSessions: this.activeSessionCount,
    })
    this.log(
      `[pod] session ready: ${sessionId} (sessions=${this.activeSessionCount}, connections=${this.activeConnectionCount})`,
    )
  }

  async teardownSession(sessionId: string, endReason?: string): Promise<void> {
    const slot = this.slots.get(sessionId)
    if (!slot) return

    const resolvedReason = endReason ?? slot.pendingEndReason
    slot.pendingEndReason = undefined
    slot.reconnectEnabled = false
    this.cancelTeardownTimer(sessionId)
    await slot.host.close()
    slot.signaling.disconnect()
    this.slots.delete(sessionId)
    this.options.onSessionChange?.({
      sessionId,
      action: 'destroyed',
      activeSessions: this.activeSessionCount,
      endReason: resolvedReason,
    })
    this.log(
      `[pod] session torn down: ${sessionId} (sessions=${this.activeSessionCount}, connections=${this.activeConnectionCount})`,
    )
  }

  private maybeScheduleIdleTeardownAfterLastPeer(sessionId: string): void {
    if (!this.teardownIdle) return
    setTimeout(() => {
      const slot = this.slots.get(sessionId)
      if (!slot || slot.host.activeClientCount > 0) return
      this.scheduleIdleTeardown(sessionId)
    }, 0)
  }

  private bindAgentSignalingReconnect(slot: SessionSlot): void {
    slot.signaling.on('disconnected', () => {
      if (!slot.reconnectEnabled) return
      void this.reconnectAgentSignaling(slot)
    })
  }

  private async reconnectAgentSignaling(slot: SessionSlot): Promise<void> {
    if (!slot.reconnectEnabled || !this.slots.has(slot.sessionId)) return
    if (slot.reconnectInFlight) return

    slot.reconnectInFlight = true
    try {
      this.log(
        `[pod] agent signaling disconnected — reconnecting session ${slot.sessionId}`,
      )
      await slot.signaling.connect()
      this.log(`[pod] agent signaling rejoined session ${slot.sessionId}`)
    } catch (error: unknown) {
      console.error(
        `Failed to reconnect agent signaling for ${slot.sessionId}:`,
        error,
      )
      if (slot.reconnectEnabled && this.slots.has(slot.sessionId)) {
        setTimeout(() => {
          void this.reconnectAgentSignaling(slot)
        }, 1_000)
      }
    } finally {
      slot.reconnectInFlight = false
    }
  }

  private wrapVoiceHandler(
    sessionId: string,
    handler?: VoiceSessionHandler,
  ): VoiceSessionHandler | undefined {
    if (!handler && !this.teardownIdle) return handler
    return {
      ...handler,
      onPeerConnected: (ctx) => {
        if (this.teardownIdle) {
          this.cancelTeardownTimer(sessionId)
        }
        return handler?.onPeerConnected?.(ctx)
      },
      onPeerDisconnected: (ctx) => {
        this.maybeScheduleIdleTeardownAfterLastPeer(sessionId)
        return handler?.onPeerDisconnected?.(ctx)
      },
      onPeerSignalingLost: (ctx) => {
        this.maybeScheduleIdleTeardownAfterLastPeer(sessionId)
        return handler?.onPeerSignalingLost?.(ctx)
      },
    }
  }

  /**
   * Disconnect one browser peer. Tears down the room when this was the last peer.
   */
  disconnectPeer(sessionId: string, peerId: string, endReason?: string): void {
    const slot = this.slots.get(sessionId)
    if (!slot) return

    slot.host.disconnectPeer(peerId)
    if (slot.host.activeClientCount === 0) {
      this.scheduleIdleTeardown(sessionId, endReason)
    }
  }

  async close(): Promise<void> {
    for (const sessionId of [...this.slots.keys()]) {
      await this.teardownSession(sessionId)
    }
    await this.signalingServer.close()
  }
}
