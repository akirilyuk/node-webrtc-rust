import { EventEmitter } from 'node:events'
import { describe, expect, it, vi } from 'vitest'

import { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'
import type { VoiceSessionHandler } from '../src/voice-session-handler.js'

type FakeSession = {
  clientHangup?: boolean
  pc: {
    connectionState: string
    close: ReturnType<typeof vi.fn>
    closeAsync?: ReturnType<typeof vi.fn>
  }
  controlChannel: { readyState: string; send: ReturnType<typeof vi.fn> }
  syncChannel?: undefined
  agent?: {
    attach: ReturnType<typeof vi.fn>
    start: ReturnType<typeof vi.fn>
    stop: ReturnType<typeof vi.fn>
    sendTextToTTS: ReturnType<typeof vi.fn>
    on: ReturnType<typeof vi.fn>
    speechEvents: () => AsyncGenerator<never, void, unknown>
  }
  agentOut?: { writeSample: ReturnType<typeof vi.fn> }
  inboundTrack?: unknown
  budgetLease: string
  agentStarted: boolean
  agentStartInProgress: boolean
  peerTransportReadyNotified: boolean
  peerConnectedNotified: boolean
  peerSignalingJoined: boolean
  unwireControl?: () => void
  unwireSync?: () => void
  unwireSpeechForward?: () => void
  remoteDescriptionSet: boolean
  offerSent: boolean
  pendingAnswer: null
  pendingIce: never[]
}

type HostTestAccess = VoiceAgentSessionHost & {
  sessions: Map<string, FakeSession>
  closingPeers: Map<string, Promise<unknown>>
  connectingPeers: Map<string, Promise<void>>
  closeClientInner: (peerId: string) => Promise<unknown>
  connectClientInner: (peerId: string) => Promise<void>
  connectClientBuildSession: (
    peerId: string,
    budgetLease: string,
    hooks: {
      onPeerCreated: (pc: unknown) => void
      onAgentCreated: (agent: unknown) => void
      onRegistered: (built: { pc: unknown; agent?: unknown }) => void
    },
  ) => Promise<void>
  hostClosing: boolean
  quarantinedLeases: Set<string>
  recycleRequired: boolean
  reconnectAttempts: Map<string, number>
}

class FakeSignaling extends EventEmitter {
  room = 'test-room'
  sendIceCandidate = vi.fn()
  sendOffer = vi.fn()
}

function createHost(
  voiceHandler: VoiceSessionHandler = {},
  budget: {
    tryAcquire: (peerId?: string) => string | null
    release: ReturnType<typeof vi.fn>
    snapshot: () => {
      active: number
      max: number
      available: number
      rejectedTotal: number
    }
  } = {
    tryAcquire: () => 'lease-default',
    release: vi.fn(),
    snapshot: () => ({ active: 0, max: 1, available: 1, rejectedTotal: 0 }),
  },
  signaling: FakeSignaling = new FakeSignaling(),
): HostTestAccess {
  const host = new VoiceAgentSessionHost(signaling as never, [], {
    voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } } as never,
    voiceHandler,
    sessionBudget: budget as never,
  })
  return host as unknown as HostTestAccess
}

function createFakeSession(overrides: Partial<FakeSession> = {}): FakeSession {
  return {
    pc: {
      connectionState: 'connected',
      close: vi.fn(),
      closeAsync: vi.fn(async () => undefined),
    },
    controlChannel: { readyState: 'open', send: vi.fn() },
    agent: {
      attach: vi.fn(async () => undefined),
      start: vi.fn(async () => undefined),
      stop: vi.fn(async () => undefined),
      sendTextToTTS: vi.fn(async () => undefined),
      on: vi.fn(() => () => undefined),
      speechEvents: async function* () {
        /* empty */
      },
    },
    budgetLease: 'lease-1',
    agentStarted: true,
    agentStartInProgress: false,
    peerTransportReadyNotified: true,
    peerConnectedNotified: true,
    peerSignalingJoined: true,
    remoteDescriptionSet: true,
    offerSent: true,
    pendingAnswer: null,
    pendingIce: [],
    ...overrides,
  }
}

type InboundReportEntry = Record<string, unknown>

function statsReport(entries: InboundReportEntry[]): Map<string, InboundReportEntry> {
  return new Map(entries.map((e, i) => [String(i), e]))
}

describe('VoiceAgentSessionHost disconnect inbound audio stats', () => {
  it('disconnect handler sees stats read before the peer connection closes', async () => {
    const order: string[] = []
    let seen: unknown = 'unset'
    const host = createHost({
      onPeerDisconnected: async (ctx) => {
        seen = await ctx.getInboundAudioStats()
      },
    })
    const getStats = vi.fn(async () => {
      order.push('getStats')
      return statsReport([
        { type: 'inbound-rtp', kind: 'audio', packetsReceived: 42, bytesReceived: 4200 },
      ])
    })
    const closeAsync = vi.fn(async () => {
      order.push('closeAsync')
    })
    host.sessions.set(
      'client-1',
      createFakeSession({
        pc: { connectionState: 'connected', close: vi.fn(), closeAsync, getStats } as never,
      }),
    )

    await host.disconnectPeer('client-1')
    await vi.waitFor(() => {
      expect(seen).not.toBe('unset')
    })
    expect(seen).toEqual({ packetsReceived: 42, bytesReceived: 4200 })
    expect(order.indexOf('getStats')).toBeGreaterThanOrEqual(0)
    expect(order.indexOf('getStats')).toBeLessThan(order.indexOf('closeAsync'))
  })

  it('a stats read that never answers delays teardown by at most 500 ms', async () => {
    vi.useFakeTimers()
    try {
      let seen: unknown = 'unset'
      const host = createHost({
        onPeerDisconnected: async (ctx) => {
          seen = await ctx.getInboundAudioStats()
        },
      })
      const closeAsync = vi.fn(async () => undefined)
      host.sessions.set(
        'client-1',
        createFakeSession({
          pc: {
            connectionState: 'connected',
            close: vi.fn(),
            closeAsync,
            getStats: () => new Promise(() => {}),
          } as never,
        }),
      )

      const closing = host.disconnectPeer('client-1')
      await vi.advanceTimersByTimeAsync(499)
      expect(closeAsync).not.toHaveBeenCalled()
      await vi.advanceTimersByTimeAsync(1)
      await closing
      expect(closeAsync).toHaveBeenCalledTimes(1)
      expect(seen).toBeNull()
    } finally {
      vi.useRealTimers()
    }
  })
})
