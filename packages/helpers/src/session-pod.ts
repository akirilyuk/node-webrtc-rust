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
import { VOICE_AGENT_SERVER_PEER_ID, VoiceAgentSessionHost } from './voice-agent-session-host.js'
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
  /** Server-side signaling peer id (default {@link VOICE_AGENT_SERVER_PEER_ID}). */
  serverPeerId?: string
  /** Shared across all rooms in this pod (default: process env budget). */
  sessionBudget?: VoiceSessionBudget
  /** Per-tab STT/TTS logic (same handler instance for every room in this pod). */
  voiceHandler?: VoiceSessionHandler
  log?: (message: string) => void
}

export interface SessionPodChangeEvent {
  sessionId: string
  action: 'created' | 'destroyed'
  activeSessions: number
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
  private readonly teardownIdle: boolean
  private readonly log: (message: string) => void
  private readonly sessionBudget: VoiceSessionBudget

  constructor(
    private readonly signalingServer: SignalingServer,
    private readonly options: SessionPodOptions,
  ) {
    this.teardownIdle = options.teardownIdleSessions ?? true
    this.log = options.log ?? ((message) => console.log(message))
    this.sessionBudget = options.sessionBudget ?? getProcessVoiceSessionBudget()
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
      sessionBudget: this.sessionBudget,
      voiceHandler: this.options.voiceHandler,
      log: this.options.log,
    })

    if (this.teardownIdle) {
      signaling.on('peer-left', (peerId) => {
        if (!peerId.startsWith('client-')) return
        if (host.activeClientCount > 0) return
        void this.teardownSession(sessionId).catch((error: unknown) => {
          console.error(`Failed to teardown idle session ${sessionId}:`, error)
        })
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

  async teardownSession(sessionId: string): Promise<void> {
    const slot = this.slots.get(sessionId)
    if (!slot) return

    await slot.host.close()
    slot.signaling.disconnect()
    this.slots.delete(sessionId)
    this.options.onSessionChange?.({
      sessionId,
      action: 'destroyed',
      activeSessions: this.activeSessionCount,
    })
    this.log(
      `[pod] session torn down: ${sessionId} (sessions=${this.activeSessionCount}, connections=${this.activeConnectionCount})`,
    )
  }

  async close(): Promise<void> {
    for (const sessionId of [...this.slots.keys()]) {
      await this.teardownSession(sessionId)
    }
    await this.signalingServer.close()
  }
}
