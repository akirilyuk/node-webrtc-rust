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
import {
  SessionPodCapacityFullError,
  SessionPodRecycleRequiredError,
} from './session-pod-errors.js'

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
  /**
   * Called after a session slot is created or destroyed (metrics, orchestrator hooks).
   * May return a Promise — teardown awaits `destroyed` so child `session_end` can
   * finish before capacity/slot release. Sync callbacks remain supported.
   */
  onSessionChange?: (event: SessionPodChangeEvent) => void | Promise<void>
  /** Hold the runner slot after the last client leaves so same-session reconnect can succeed. */
  rejoinGraceMs?: number
  /**
   * Longer idle grace when a peer leaves before transport was ready (signaling-only join).
   * Gives half-open / slow DTLS reconnect time to find voice-agent-server.
   */
  neverConnectedRejoinGraceMs?: number
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

/** Grace when the last peer left before WebRTC transport was ready (pre-DTLS reconnect). */
export const DEFAULT_NEVER_CONNECTED_REJOIN_GRACE_MS = 60_000

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
export type SessionPodCloseOutcome = {
  recycleRequired: boolean
  quarantined: number
  failures: unknown[]
}

export class SessionPod {
  private readonly slots = new Map<string, SessionSlot>()
  /** Sessions mid-prepare (after capacity check, before slot is committed). */
  private readonly preparingSessions = new Set<string>()
  private readonly teardownTimers = new Map<string, ReturnType<typeof setTimeout>>()
  /** Single-flight teardown per session (idle / forced / drain share one destroy). */
  private readonly teardownFlights = new Map<string, Promise<void>>()
  /**
   * Hosts retired from `slots` that still hold quarantined leases.
   * Keeps recycle/quarantine visible after teardownSession deletes the slot.
   */
  private readonly retiredHosts = new Map<string, VoiceAgentSessionHost>()
  private readonly teardownIdle: boolean
  private readonly rejoinGraceMs: number
  private readonly neverConnectedRejoinGraceMs: number
  private readonly log: (message: string) => void
  private readonly sessionBudget: VoiceSessionBudget

  constructor(
    private readonly signalingServer: SignalingServer,
    private readonly options: SessionPodOptions,
  ) {
    this.teardownIdle = options.teardownIdleSessions ?? true
    this.rejoinGraceMs = options.rejoinGraceMs ?? DEFAULT_SESSION_REJOIN_GRACE_MS
    this.neverConnectedRejoinGraceMs =
      options.neverConnectedRejoinGraceMs ?? DEFAULT_NEVER_CONNECTED_REJOIN_GRACE_MS
    this.log = options.log ?? ((message) => console.log(message))
    this.sessionBudget = options.sessionBudget ?? getProcessVoiceSessionBudget()
  }

  private pruneRetiredHosts(): void {
    for (const [sessionId, host] of this.retiredHosts) {
      if (!host.isRecycleRequired && host.quarantinedCount === 0) {
        this.retiredHosts.delete(sessionId)
      }
    }
  }

  private cancelTeardownTimer(sessionId: string): void {
    const timer = this.teardownTimers.get(sessionId)
    if (timer) {
      clearTimeout(timer)
      this.teardownTimers.delete(sessionId)
    }
  }

  private scheduleIdleTeardown(
    sessionId: string,
    endReason?: string,
    graceMs: number = this.rejoinGraceMs,
  ): void {
    if (!this.teardownIdle) return
    const slot = this.slots.get(sessionId)
    if (slot && endReason) {
      slot.pendingEndReason = endReason
    }
    // WebRTC close and signaling peer-left both notify SessionPod — arm grace once.
    if (this.teardownTimers.has(sessionId)) {
      return
    }
    if (graceMs <= 0) {
      void this.teardownSession(sessionId, endReason).catch((error: unknown) => {
        console.error(`Failed to teardown idle session ${sessionId}:`, error)
      })
      return
    }
    const timer = setTimeout(() => {
      this.teardownTimers.delete(sessionId)
      const current = this.slots.get(sessionId)
      if (!current || current.host.activeClientCount > 0) return
      void this.teardownSession(sessionId, current.pendingEndReason ?? endReason).catch(
        (error: unknown) => {
          console.error(`Failed to teardown idle session ${sessionId}:`, error)
        },
      )
    }, graceMs)
    this.teardownTimers.set(sessionId, timer)
    this.log(`[pod] session ${sessionId} idle — teardown in ${graceMs}ms unless client rejoins`)
  }

  get sessionBudgetSnapshot(): VoiceSessionBudgetSnapshot {
    this.pruneRetiredHosts()
    const snap = this.sessionBudget.snapshot()
    return {
      ...snap,
      quarantined: this.quarantinedPeerCount,
      recycleRequired: this.isRecycleRequired,
    }
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

  /** Live + retired host quarantines (capacity occupied, not placeable). */
  get quarantinedPeerCount(): number {
    this.pruneRetiredHosts()
    let total = 0
    for (const slot of this.slots.values()) {
      total += slot.host.quarantinedCount
    }
    for (const host of this.retiredHosts.values()) {
      total += host.quarantinedCount
    }
    return total
  }

  /** True when any live or retired host requires recycle / is non-assignable. */
  get isRecycleRequired(): boolean {
    this.pruneRetiredHosts()
    for (const slot of this.slots.values()) {
      if (slot.host.isRecycleRequired) return true
    }
    for (const host of this.retiredHosts.values()) {
      if (host.isRecycleRequired) return true
    }
    return false
  }

  listSessions(): SessionPodSessionInfo[] {
    return [...this.slots.values()].map((slot) => ({
      sessionId: slot.sessionId,
      connections: slot.host.activeClientCount,
    }))
  }

  async ensureSession(sessionId: string): Promise<void> {
    if (this.slots.has(sessionId)) return
    if (this.preparingSessions.has(sessionId)) return

    if (this.isRecycleRequired) {
      const quarantined = this.quarantinedPeerCount
      this.log(
        `[pod] session prepare rejected — recycle required (quarantined=${quarantined}) sessionId=${sessionId}; orchestrator should not assign here`,
      )
      throw new SessionPodRecycleRequiredError(quarantined)
    }

    const maxPrepared = this.options.maxPreparedSessions ?? 0
    this.preparingSessions.add(sessionId)
    try {
      if (maxPrepared > 0 && this.occupiedPrepareSlots() > maxPrepared) {
        this.log(
          `[pod] session prepare rejected — slot capacity full (${this.slots.size}/${maxPrepared}) sessionId=${sessionId}; orchestrator should not assign here`,
        )
        throw new SessionPodCapacityFullError(this.slots.size, maxPrepared)
      }

      await this.prepareSessionSlot(sessionId)
    } finally {
      this.preparingSessions.delete(sessionId)
    }
  }

  private occupiedPrepareSlots(): number {
    return this.slots.size + this.preparingSessions.size
  }

  private async prepareSessionSlot(sessionId: string): Promise<void> {
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
    await Promise.resolve(
      this.options.onSessionChange?.({
        sessionId,
        action: 'created',
        activeSessions: this.activeSessionCount,
      }),
    )
    this.log(
      `[pod] session ready: ${sessionId} (sessions=${this.activeSessionCount}, connections=${this.activeConnectionCount})`,
    )
  }

  async teardownSession(sessionId: string, endReason?: string): Promise<void> {
    const inFlight = this.teardownFlights.get(sessionId)
    if (inFlight) {
      if (endReason) {
        const slot = this.slots.get(sessionId)
        if (slot && !slot.pendingEndReason) {
          slot.pendingEndReason = endReason
        }
      }
      return inFlight
    }
    // Already torn down (including sticky retired quarantine) — no second destroy event.
    if (!this.slots.has(sessionId)) {
      return
    }

    const flight = this.teardownSessionOnce(sessionId, endReason).finally(() => {
      this.teardownFlights.delete(sessionId)
    })
    this.teardownFlights.set(sessionId, flight)
    return flight
  }

  private async teardownSessionOnce(sessionId: string, endReason?: string): Promise<void> {
    const slot = this.slots.get(sessionId)
    if (!slot) return

    const resolvedReason = endReason ?? slot.pendingEndReason
    slot.pendingEndReason = undefined
    slot.reconnectEnabled = false
    this.cancelTeardownTimer(sessionId)
    await slot.host.close()
    slot.signaling.disconnect()
    // Keep slot in `slots` while awaiting destroyed so concurrent heartbeat/getters
    // cannot observe capacity freed mid-hook. Pass prospective count (size - 1).
    // Do NOT move host into retiredHosts before the hook — rejection must retain
    // the host solely as a live slot (no double-count with retiredHosts).
    const prospectiveActiveSessions = Math.max(0, this.slots.size - 1)
    try {
      await Promise.resolve(
        this.options.onSessionChange?.({
          sessionId,
          action: 'destroyed',
          activeSessions: prospectiveActiveSessions,
          endReason: resolvedReason,
        }),
      )
    } catch (error: unknown) {
      // Fail-closed: retain slot/capacity so a retry can still find the session.
      this.log(
        `[pod] session destroyed hook failed — retaining slot ${sessionId}: ${
          error instanceof Error ? error.message : String(error)
        }`,
      )
      throw error
    }
    // Persist quarantine only after destroyed succeeds, with slot deletion.
    if (slot.host.isRecycleRequired || slot.host.quarantinedCount > 0) {
      this.retiredHosts.set(sessionId, slot.host)
      this.log(
        `[pod] session ${sessionId} retired with quarantine (quarantined=${slot.host.quarantinedCount}) — recycle required until convergence or process recycle`,
      )
    }
    this.slots.delete(sessionId)
    this.log(
      `[pod] session torn down: ${sessionId} (sessions=${this.activeSessionCount}, connections=${this.activeConnectionCount}, recycleRequired=${this.isRecycleRequired})`,
    )
  }

  /**
   * Schedule idle teardown only after the host has no live, connecting, or closing peers.
   * `onPeerDisconnected` runs mid-close while the peer is still counted as active.
   */
  private maybeScheduleIdleTeardownAfterLastPeer(
    sessionId: string,
    graceMs: number = this.rejoinGraceMs,
  ): void {
    if (!this.teardownIdle) return
    const deadline = Date.now() + 15_000
    const poll = (): void => {
      const slot = this.slots.get(sessionId)
      if (!slot) return
      if (slot.host.activeClientCount > 0) {
        if (Date.now() < deadline) {
          setTimeout(poll, 25)
        }
        return
      }
      this.scheduleIdleTeardown(sessionId, undefined, graceMs)
    }
    setTimeout(poll, 0)
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
      this.log(`[pod] agent signaling disconnected — reconnecting session ${slot.sessionId}`)
      await slot.signaling.connect()
      this.log(`[pod] agent signaling rejoined session ${slot.sessionId}`)
    } catch (error: unknown) {
      console.error(`Failed to reconnect agent signaling for ${slot.sessionId}:`, error)
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
      onPeerTransportReady: (ctx) => {
        if (this.teardownIdle) {
          this.cancelTeardownTimer(sessionId)
        }
        return handler?.onPeerTransportReady?.(ctx)
      },
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
        this.maybeScheduleIdleTeardownAfterLastPeer(sessionId, this.neverConnectedRejoinGraceMs)
        return handler?.onPeerSignalingLost?.(ctx)
      },
    }
  }

  /**
   * Disconnect one browser peer. Tears down the room when this was the last peer.
   * Awaits host peer cleanup (native close) before checking idle teardown.
   */
  async disconnectPeer(sessionId: string, peerId: string, endReason?: string): Promise<void> {
    const slot = this.slots.get(sessionId)
    if (!slot) return

    await slot.host.disconnectPeer(peerId)
    if (slot.host.activeClientCount === 0) {
      this.scheduleIdleTeardown(sessionId, endReason)
    }
  }

  async close(): Promise<SessionPodCloseOutcome> {
    const failures: unknown[] = []
    const sessionIds = [...this.slots.keys()]
    await Promise.all(
      sessionIds.map(async (sessionId) => {
        try {
          await this.teardownSession(sessionId)
        } catch (error: unknown) {
          failures.push(error)
          console.error(`Failed to teardown session ${sessionId} during pod close:`, error)
        }
      }),
    )
    try {
      await this.signalingServer.close()
    } catch (error: unknown) {
      failures.push(error)
    }
    const outcome: SessionPodCloseOutcome = {
      recycleRequired: this.isRecycleRequired,
      quarantined: this.quarantinedPeerCount,
      failures,
    }
    if (outcome.recycleRequired) {
      this.log(
        `[pod] close complete with recycle required (quarantined=${outcome.quarantined}, failures=${failures.length})`,
      )
    }
    return outcome
  }
}
