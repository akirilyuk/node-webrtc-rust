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

interface SessionSlot {
  sessionId: string
  signaling: SignalingClient
  host: VoiceAgentSessionHost
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
    this.rejoinGraceMs = options.rejoinGraceMs ?? 90_000
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

  private scheduleIdleTeardown(sessionId: string): void {
    if (!this.teardownIdle) return
    this.cancelTeardownTimer(sessionId)
    if (this.rejoinGraceMs <= 0) {
      void this.teardownSession(sessionId).catch((error: unknown) => {
        console.error(`Failed to teardown idle session ${sessionId}:`, error)
      })
      return
    }
    const timer = setTimeout(() => {
      this.teardownTimers.delete(sessionId)
      const slot = this.slots.get(sessionId)
      if (!slot || slot.host.activeClientCount > 0) return
      void this.teardownSession(sessionId).catch((error: unknown) => {
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

    const serverPeerId = this.options.serverPeerId ?? VOICE_AGENT_SERVER_PEER_ID
    const signaling = new SignalingClient({
      url: this.options.signalingUrl,
      room: sessionId,
      peerId: serverPeerId,
    })
    await signaling.connect()

    const host = new VoiceAgentSessionHost(signaling, this.options.iceServers, {
      voiceConfig: this.options.voiceConfig,
      sessionMode: this.options.sessionMode,
      sessionBudget: this.sessionBudget,
      voiceHandler: this.options.voiceHandler,
      syncChannel: this.options.syncChannel,
      log: this.options.log,
    })

    if (this.teardownIdle) {
      signaling.on('peer-joined', (peerId) => {
        if (!peerId.startsWith('client-')) return
        this.cancelTeardownTimer(sessionId)
      })
      signaling.on('peer-left', (peerId) => {
        if (!peerId.startsWith('client-')) return
        if (host.activeClientCount > 0) return
        this.scheduleIdleTeardown(sessionId)
      })
    }

    this.slots.set(sessionId, { sessionId, signaling, host })
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

    this.cancelTeardownTimer(sessionId)
    await slot.host.close()
    slot.signaling.disconnect()
    this.slots.delete(sessionId)
    this.options.onSessionChange?.({
      sessionId,
      action: 'destroyed',
      activeSessions: this.activeSessionCount,
      endReason,
    })
    this.log(
      `[pod] session torn down: ${sessionId} (sessions=${this.activeSessionCount}, connections=${this.activeConnectionCount})`,
    )
  }

  /**
   * Disconnect one browser peer. Tears down the room when this was the last peer.
   */
  disconnectPeer(sessionId: string, peerId: string, endReason?: string): void {
    const slot = this.slots.get(sessionId)
    if (!slot) return

    slot.host.disconnectPeer(peerId)
    if (slot.host.activeClientCount === 0) {
      void this.teardownSession(sessionId, endReason).catch((error: unknown) => {
        console.error(
          `Failed to teardown session ${sessionId} after peer disconnect:`,
          error,
        )
      })
    }
  }

  async close(): Promise<void> {
    for (const sessionId of [...this.slots.keys()]) {
      await this.teardownSession(sessionId)
    }
    await this.signalingServer.close()
  }
}
