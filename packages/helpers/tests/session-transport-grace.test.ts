import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  DEFAULT_PEER_TRANSPORT_DISCONNECT_GRACE_MS,
  VoiceAgentSessionHost,
} from '../src/voice-agent-session-host.js'
import { DEFAULT_SESSION_REJOIN_GRACE_MS, SessionPod } from '../src/session-pod.js'
import type { VoiceSessionHandler } from '../src/voice-session-handler.js'

type FakePc = {
  connectionState: string
  iceConnectionState: string
  close: ReturnType<typeof vi.fn>
}

type FakeSession = {
  pc: FakePc
  controlChannel: { readyState: string; send: ReturnType<typeof vi.fn> }
  budgetLease: string
  agentStarted: boolean
  agentStartInProgress: boolean
  peerTransportReadyNotified: boolean
  peerConnectedNotified: boolean
  peerSignalingJoined: boolean
  remoteDescriptionSet: boolean
  offerSent: boolean
  pendingAnswer: null
  pendingIce: never[]
  transportDisconnectTimer?: ReturnType<typeof setTimeout>
  transportDownSince?: number
}

type HostAccess = VoiceAgentSessionHost & {
  sessions: Map<string, FakeSession>
  scheduleTransportDisconnect: (peerId: string, session: FakeSession) => void
  noteTransportRestored: (
    peerId: string,
    session: FakeSession,
    via: 'ice-restart' | 'rejoin',
  ) => void
  connectClientInner: (peerId: string) => Promise<void>
}

function createFakeSession(state = 'disconnected'): FakeSession {
  return {
    pc: { connectionState: state, iceConnectionState: state, close: vi.fn() },
    controlChannel: { readyState: 'open', send: vi.fn() },
    budgetLease: 'lease-test',
    agentStarted: false,
    agentStartInProgress: false,
    peerTransportReadyNotified: true,
    peerConnectedNotified: false,
    peerSignalingJoined: true,
    remoteDescriptionSet: true,
    offerSent: true,
    pendingAnswer: null,
    pendingIce: [],
  }
}

function createHost(
  opts: {
    voiceHandler?: VoiceSessionHandler
    transportDisconnectGraceMs?: number
    logs?: string[]
    handlers?: Map<string, (...args: never[]) => void>
  } = {},
): HostAccess {
  const handlers = opts.handlers ?? new Map()
  const signaling = {
    room: 'test-room',
    on: vi.fn((event: string, cb: (...args: never[]) => void) => {
      handlers.set(event, cb)
    }),
  }
  const host = new VoiceAgentSessionHost(signaling as never, [], {
    voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } } as never,
    sessionMode: 'data-only',
    voiceHandler: opts.voiceHandler,
    transportDisconnectGraceMs: opts.transportDisconnectGraceMs,
    log: opts.logs ? (m) => opts.logs!.push(m) : () => undefined,
    sessionBudget: {
      tryAcquire: () => 'lease-test',
      release: () => undefined,
      snapshot: () => ({ active: 0, max: 0, available: 0, rejectedTotal: 0 }),
    },
  })
  return host as unknown as HostAccess
}

describe('transport disconnect grace', () => {
  afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('keeps a peer whose transport is down for 6 s and resumes on reconnected', async () => {
    vi.useFakeTimers()
    const logs: string[] = []
    const host = createHost({ logs })
    const session = createFakeSession()
    host.sessions.set('client-1', session)

    host.scheduleTransportDisconnect('client-1', session)
    await vi.advanceTimersByTimeAsync(6_000)

    session.pc.connectionState = 'connected'
    session.pc.iceConnectionState = 'connected'
    host.noteTransportRestored('client-1', session, 'ice-restart')
    await vi.advanceTimersByTimeAsync(60_000)

    expect(host.sessions.get('client-1')).toBe(session)
    expect(session.pc.close).not.toHaveBeenCalled()
    expect(vi.getTimerCount()).toBe(0)
    expect(logs).toContain(
      `[data client-1] transport down (disconnected) — keeping session for ${DEFAULT_PEER_TRANSPORT_DISCONNECT_GRACE_MS}ms`,
    )
    expect(logs).toContain('[data client-1] transport restored after 6000ms (ice-restart)')
  })

  it('closes the peer after the transport grace (30 s) and reaps after the rejoin grace (15 s)', async () => {
    vi.useFakeTimers()
    const logs: string[] = []
    const pod = new SessionPod({} as never, {
      signalingUrl: 'ws://127.0.0.1/ws',
      iceServers: [],
      voiceConfig: {} as never,
      teardownIdleSessions: true,
    }) as unknown as {
      wrapVoiceHandler: (id: string, h?: VoiceSessionHandler) => VoiceSessionHandler | undefined
      slots: Map<string, unknown>
      teardownSession: (id: string, reason?: string) => Promise<void>
    }
    const teardown = vi.spyOn(pod, 'teardownSession').mockResolvedValue(undefined)
    const host = createHost({ logs, voiceHandler: pod.wrapVoiceHandler('session-1') })
    pod.slots.set('session-1', { sessionId: 'session-1', host })

    const session = createFakeSession()
    host.sessions.set('client-1', session)
    host.scheduleTransportDisconnect('client-1', session)

    expect(DEFAULT_PEER_TRANSPORT_DISCONNECT_GRACE_MS).toBe(30_000)
    expect(DEFAULT_SESSION_REJOIN_GRACE_MS).toBe(15_000)

    await vi.advanceTimersByTimeAsync(29_999)
    expect(host.sessions.has('client-1')).toBe(true)
    expect(session.pc.close).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(1)
    expect(host.sessions.has('client-1')).toBe(false)
    expect(session.pc.close).toHaveBeenCalledTimes(1)
    expect(logs).toContain('[data client-1] transport still down after 30000ms — closing peer')
    expect(teardown).not.toHaveBeenCalled()

    // The pod's idle poll (`setTimeout(poll, 0)`, 1 ms in Node) arms the rejoin grace right
    // after the close, so the reap lands at 30_001 + 15_000.
    await vi.advanceTimersByTimeAsync(15_000)
    expect(teardown).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(1)
    expect(teardown).toHaveBeenCalledTimes(1)
    expect(teardown.mock.calls[0]?.[0]).toBe('session-1')
  })

  it('same-session rejoin during transport grace cancels the disconnect timer and replaces the peer', async () => {
    vi.useFakeTimers()
    const logs: string[] = []
    const handlers = new Map<string, (...args: never[]) => void>()
    const host = createHost({ logs, handlers })
    const connect = vi.spyOn(host, 'connectClientInner').mockResolvedValue(undefined)
    const stale = createFakeSession()
    host.sessions.set('client-1', stale)
    host.scheduleTransportDisconnect('client-1', stale)
    expect(vi.getTimerCount()).toBe(1)

    await vi.advanceTimersByTimeAsync(4_000)
    ;(handlers.get('peer-joined') as (peerId: string) => void)('client-1')
    await vi.advanceTimersByTimeAsync(0)

    expect(stale.pc.close).toHaveBeenCalledTimes(1)
    expect(stale.transportDisconnectTimer).toBeUndefined()
    expect(connect).toHaveBeenCalledTimes(1)
    expect(vi.getTimerCount()).toBe(0)
    expect(logs).toContain('[data client-1] transport restored after 4000ms (rejoin)')

    await vi.advanceTimersByTimeAsync(60_000)
    expect(connect).toHaveBeenCalledTimes(1)
  })

  it('transportDisconnectGraceMs option overrides the default', async () => {
    vi.useFakeTimers()
    const logs: string[] = []
    const host = createHost({ logs, transportDisconnectGraceMs: 2_000 })
    const session = createFakeSession()
    host.sessions.set('client-1', session)
    host.scheduleTransportDisconnect('client-1', session)

    await vi.advanceTimersByTimeAsync(1_999)
    expect(host.sessions.has('client-1')).toBe(true)

    await vi.advanceTimersByTimeAsync(1)
    expect(host.sessions.has('client-1')).toBe(false)
    expect(logs).toContain('[data client-1] transport still down after 2000ms — closing peer')
  })
})
