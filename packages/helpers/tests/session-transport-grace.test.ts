import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  DEFAULT_PEER_TRANSPORT_DISCONNECT_GRACE_MS,
  DEFAULT_TRANSPORT_REOFFER_AFTER_MS,
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
  transportReofferTimer?: ReturnType<typeof setTimeout>
  signalingLeft?: boolean
  clientHangup?: boolean
}

type HostAccess = VoiceAgentSessionHost & {
  sessions: Map<string, FakeSession>
  scheduleTransportDisconnect: (peerId: string, session: FakeSession) => void
  noteTransportRestored: (
    peerId: string,
    session: FakeSession,
    via: 'ice-restart' | 'rejoin',
  ) => void
  connectClientInner: (peerId: string, opts?: { reoffer?: boolean }) => Promise<void>
  closeClientInner: (peerId: string) => Promise<void>
  installClientHangupInterceptor: (
    peerId: string,
    session: FakeSession,
    channel: { onmessage: ((event: { data: unknown }) => void) | null },
  ) => void
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
    transportReofferAfterMs?: number
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
    sendOffer: vi.fn(),
    sendIceCandidate: vi.fn(),
  }
  const host = new VoiceAgentSessionHost(signaling as never, [], {
    voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } } as never,
    sessionMode: 'data-only',
    voiceHandler: opts.voiceHandler,
    transportDisconnectGraceMs: opts.transportDisconnectGraceMs,
    transportReofferAfterMs: opts.transportReofferAfterMs,
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
    const host = createHost({ logs, transportReofferAfterMs: 0 })
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

  it('closes the peer after the transport grace (10 s) and reaps after the rejoin grace (5 s)', async () => {
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
    const host = createHost({
      logs,
      voiceHandler: pod.wrapVoiceHandler('session-1'),
      transportReofferAfterMs: 0,
    })
    pod.slots.set('session-1', { sessionId: 'session-1', host })

    const session = createFakeSession()
    host.sessions.set('client-1', session)
    host.scheduleTransportDisconnect('client-1', session)

    expect(DEFAULT_PEER_TRANSPORT_DISCONNECT_GRACE_MS).toBe(10_000)
    expect(DEFAULT_SESSION_REJOIN_GRACE_MS).toBe(5_000)

    await vi.advanceTimersByTimeAsync(9_999)
    expect(host.sessions.has('client-1')).toBe(true)
    expect(session.pc.close).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(1)
    expect(host.sessions.has('client-1')).toBe(false)
    expect(session.pc.close).toHaveBeenCalledTimes(1)
    expect(logs).toContain('[data client-1] transport still down after 10000ms — closing peer')
    expect(teardown).not.toHaveBeenCalled()

    // The pod's idle poll (`setTimeout(poll, 0)`, 1 ms in Node) arms the rejoin grace right
    // after the close, so the reap lands at 10_001 + 5_000.
    await vi.advanceTimersByTimeAsync(5_000)
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
    expect(vi.getTimerCount()).toBe(2)

    await vi.advanceTimersByTimeAsync(2_000)
    ;(handlers.get('peer-joined') as (peerId: string) => void)('client-1')
    await vi.advanceTimersByTimeAsync(0)

    expect(stale.pc.close).toHaveBeenCalledTimes(1)
    expect(stale.transportDisconnectTimer).toBeUndefined()
    expect(connect).toHaveBeenCalledTimes(1)
    expect(vi.getTimerCount()).toBe(0)
    expect(logs).toContain('[data client-1] transport restored after 2000ms (rejoin)')

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
  /** Connects a real (never-negotiated) data-only peer and lets the test force its states. */
  async function connectRealPeer(host: HostAccess, peerId: string, opts?: { reoffer?: boolean }) {
    await host.connectClientInner(peerId, opts)
    const session = host.sessions.get(peerId)!
    const state = { ice: 'connected', conn: 'connected' }
    Object.defineProperty(session.pc, 'iceConnectionState', { get: () => state.ice })
    Object.defineProperty(session.pc, 'connectionState', { get: () => state.conn })
    const pc = session.pc as unknown as {
      oniceconnectionstatechange: () => void
      onconnectionstatechange: () => void
      close: () => void
    }
    return { session, state, pc }
  }

  it('ICE failed keeps the peer until the transport grace', async () => {
    vi.useFakeTimers()
    const host = createHost({ transportReofferAfterMs: 0 })
    const { session, state, pc } = await connectRealPeer(host, 'client-1')

    state.ice = 'disconnected'
    pc.oniceconnectionstatechange()
    await vi.advanceTimersByTimeAsync(4_000)
    state.ice = 'failed'
    pc.oniceconnectionstatechange()

    await vi.advanceTimersByTimeAsync(5_999)
    expect(host.sessions.get('client-1')).toBe(session)

    await vi.advanceTimersByTimeAsync(1)
    expect(host.sessions.has('client-1')).toBe(false)
  })

  it('an ICE state change logs one ice_pair line', async () => {
    vi.useFakeTimers()
    const logs: string[] = []
    const host = createHost({ logs })
    const { session, pc } = await connectRealPeer(host, 'client-1')
    const getStats = vi.fn(async () => new Map<string, Record<string, unknown>>())
    Object.defineProperty(session.pc, 'getStats', { value: getStats })

    pc.oniceconnectionstatechange()
    await vi.advanceTimersByTimeAsync(0)

    expect(logs.filter((l) => l.includes('ice=connected ice_pair'))).toEqual([
      '[data client-1] ice=connected ice_pair none pairs=0 states=',
    ])
  })

  it('state changes while an ICE pair read is pending start only one read', async () => {
    vi.useFakeTimers()
    const host = createHost()
    const { session, pc } = await connectRealPeer(host, 'client-1')
    const getStats = vi.fn(() => new Promise<never>(() => {}))
    Object.defineProperty(session.pc, 'getStats', { value: getStats })

    pc.oniceconnectionstatechange()
    pc.oniceconnectionstatechange()
    await vi.advanceTimersByTimeAsync(100)

    expect(getStats).toHaveBeenCalledTimes(1)
  })

  it('connectionState closed closes the peer immediately', async () => {
    vi.useFakeTimers()
    const host = createHost()
    const { state, pc } = await connectRealPeer(host, 'client-1')

    state.conn = 'closed'
    pc.onconnectionstatechange()
    await vi.advanceTimersByTimeAsync(0)

    expect(host.sessions.has('client-1')).toBe(false)
  })

  it('peer-left with transport connected keeps the peer', async () => {
    vi.useFakeTimers()
    const logs: string[] = []
    const handlers = new Map<string, (...args: never[]) => void>()
    const host = createHost({ logs, handlers })
    const session = createFakeSession('connected')
    host.sessions.set('client-1', session)
    ;(handlers.get('peer-left') as (peerId: string) => void)('client-1')
    await vi.advanceTimersByTimeAsync(60_000)

    expect(host.sessions.get('client-1')).toBe(session)
    expect(session.pc.close).not.toHaveBeenCalled()
    expect(logs).toContain('[voice client-1] signaling left but transport is up — keeping peer')
  })

  it('peer-left with transport disconnected closes the peer after the grace, not before', async () => {
    vi.useFakeTimers()
    const handlers = new Map<string, (...args: never[]) => void>()
    const host = createHost({ handlers })
    const session = createFakeSession('disconnected')
    host.sessions.set('client-1', session)
    ;(handlers.get('peer-left') as (peerId: string) => void)('client-1')
    await vi.advanceTimersByTimeAsync(9_999)
    expect(host.sessions.get('client-1')).toBe(session)

    await vi.advanceTimersByTimeAsync(1)
    expect(host.sessions.has('client-1')).toBe(false)
    expect(session.pc.close).toHaveBeenCalledTimes(1)
  })

  it('client_hangup on the control channel closes the peer immediately', async () => {
    vi.useFakeTimers()
    const host = createHost()
    const session = createFakeSession('connected')
    host.sessions.set('client-1', session)
    const passthrough = vi.fn()
    const channel = { onmessage: passthrough as ((event: { data: unknown }) => void) | null }
    host.installClientHangupInterceptor('client-1', session, channel)

    channel.onmessage!({ data: JSON.stringify({ type: 'other' }) })
    expect(passthrough).toHaveBeenCalledTimes(1)
    expect(host.sessions.has('client-1')).toBe(true)

    channel.onmessage!({ data: JSON.stringify({ type: 'client_hangup' }) })
    await vi.advanceTimersByTimeAsync(0)

    expect(session.clientHangup).toBe(true)
    expect(host.sessions.has('client-1')).toBe(false)
    expect(session.pc.close).toHaveBeenCalledTimes(1)
    expect(passthrough).toHaveBeenCalledTimes(1)
  })
  it('remote close of the control channel closes the peer immediately', async () => {
    vi.useFakeTimers()
    const logs: string[] = []
    const host = createHost({ logs })
    const { session } = await connectRealPeer(host, 'client-1')
    const channel = session.controlChannel as unknown as { onclose: (() => void) | null }

    channel.onclose?.()
    await vi.advanceTimersByTimeAsync(0)

    expect(session.clientHangup).toBe(true)
    expect(host.sessions.has('client-1')).toBe(false)
    expect(logs).toContain('[data client-1] control channel closed by client — closing peer')
  })

  it('local closeClientInner does not count as a client hangup', async () => {
    vi.useFakeTimers()
    const logs: string[] = []
    const host = createHost({ logs })
    const { session } = await connectRealPeer(host, 'client-1')
    const channel = session.controlChannel as unknown as { onclose: (() => void) | null }

    await host.disconnectPeer('client-1')
    // The native close may report the channel closing after we removed the session.
    channel.onclose?.()
    await vi.advanceTimersByTimeAsync(0)

    expect(session.clientHangup).toBeUndefined()
    expect(logs.some((m) => m.includes('closed by client'))).toBe(false)
    expect(host.sessions.has('client-1')).toBe(false)
  })

  it('a hangup tears the pod session down without the rejoin grace', async () => {
    vi.useFakeTimers()
    const pod = new SessionPod({} as never, {
      signalingUrl: 'ws://127.0.0.1/ws',
      iceServers: [],
      voiceConfig: {} as never,
      teardownIdleSessions: true,
    }) as unknown as {
      wrapVoiceHandler: (id: string, h?: VoiceSessionHandler) => VoiceSessionHandler | undefined
      slots: Map<string, unknown>
      scheduleIdleTeardown: (id: string, reason?: string, graceMs?: number) => void
      teardownSession: (id: string, reason?: string) => Promise<void>
    }
    pod.slots.set('session-1', { sessionId: 'session-1', host: { activeClientCount: 0 } })
    const schedule = vi.spyOn(pod, 'scheduleIdleTeardown')
    const teardown = vi.spyOn(pod, 'teardownSession').mockResolvedValue(undefined)
    const wrapped = pod.wrapVoiceHandler('session-1')

    wrapped?.onPeerDisconnected?.({ peerId: 'client-1' } as never, 'hangup')
    await vi.advanceTimersByTimeAsync(1)

    expect(schedule).toHaveBeenCalledWith('session-1', undefined, 0)
    expect(teardown).toHaveBeenCalledTimes(1)

    schedule.mockClear()
    wrapped?.onPeerDisconnected?.({ peerId: 'client-1' } as never, 'transport')
    await vi.advanceTimersByTimeAsync(1)
    expect(schedule).toHaveBeenCalledWith('session-1', undefined, DEFAULT_SESSION_REJOIN_GRACE_MS)
  })

  describe('re-offer after transport loss', () => {
    it('transport down with signaling joined re-offers a replacement after 3 s', async () => {
      vi.useFakeTimers()
      const logs: string[] = []
      const host = createHost({ logs })
      const connect = vi.spyOn(host, 'connectClientInner').mockResolvedValue(undefined)
      const close = vi.spyOn(host, 'closeClientInner')
      const session = createFakeSession()
      host.sessions.set('client-1', session)
      host.scheduleTransportDisconnect('client-1', session)

      expect(DEFAULT_TRANSPORT_REOFFER_AFTER_MS).toBe(3_000)
      await vi.advanceTimersByTimeAsync(2_999)
      expect(connect).not.toHaveBeenCalled()

      await vi.advanceTimersByTimeAsync(1)
      expect(close).toHaveBeenCalledTimes(1)
      expect(connect).toHaveBeenCalledTimes(1)
      expect(connect).toHaveBeenCalledWith('client-1', { reoffer: true })
      expect(close.mock.invocationCallOrder[0]).toBeLessThan(connect.mock.invocationCallOrder[0]!)
      expect(session.pc.close).toHaveBeenCalledTimes(1)
      expect(logs).toContain(
        '[data client-1] transport down for 3000ms with signaling joined — re-offering a replacement session',
      )
    })

    it('no re-offer when the client already left signaling', async () => {
      vi.useFakeTimers()
      const logs: string[] = []
      const host = createHost({ logs })
      const connect = vi.spyOn(host, 'connectClientInner').mockResolvedValue(undefined)
      const close = vi.spyOn(host, 'closeClientInner')
      const session = createFakeSession()
      session.signalingLeft = true
      host.sessions.set('client-1', session)
      host.scheduleTransportDisconnect('client-1', session)

      await vi.advanceTimersByTimeAsync(3_000)
      expect(connect).not.toHaveBeenCalled()
      expect(close).not.toHaveBeenCalled()
      expect(logs).toContain(
        '[data client-1] transport down — not re-offering (client left signaling)',
      )

      await vi.advanceTimersByTimeAsync(7_000)
      expect(host.sessions.has('client-1')).toBe(false)
      expect(session.pc.close).toHaveBeenCalledTimes(1)
      expect(connect).not.toHaveBeenCalled()
    })

    it('peer-left marks the session as off signaling, so no re-offer follows', async () => {
      vi.useFakeTimers()
      const handlers = new Map<string, (...args: never[]) => void>()
      const host = createHost({ handlers })
      const connect = vi.spyOn(host, 'connectClientInner').mockResolvedValue(undefined)
      const session = createFakeSession()
      host.sessions.set('client-1', session)
      ;(handlers.get('peer-left') as (peerId: string) => void)('client-1')

      await vi.advanceTimersByTimeAsync(3_000)
      expect(session.signalingLeft).toBe(true)
      expect(session.peerSignalingJoined).toBe(true)
      expect(connect).not.toHaveBeenCalled()
    })

    it('peer-left on a pre-transport session still calls onPeerSignalingLost on close', async () => {
      vi.useFakeTimers()
      const handlers = new Map<string, (...args: never[]) => void>()
      const onPeerSignalingLost = vi.fn()
      const onPeerDisconnected = vi.fn()
      const host = createHost({
        handlers,
        voiceHandler: { onPeerSignalingLost, onPeerDisconnected } as VoiceSessionHandler,
      })
      const session = createFakeSession('connecting')
      session.peerTransportReadyNotified = false
      host.sessions.set('client-1', session)
      ;(handlers.get('peer-left') as (peerId: string) => void)('client-1')
      expect(session.signalingLeft).toBe(true)

      await host.disconnectPeer('client-1')
      await vi.advanceTimersByTimeAsync(0)

      expect(onPeerSignalingLost).toHaveBeenCalledTimes(1)
      expect(onPeerDisconnected).not.toHaveBeenCalled()
    })

    it('no re-offer when the transport recovers within 3 s', async () => {
      vi.useFakeTimers()
      const host = createHost()
      const connect = vi.spyOn(host, 'connectClientInner').mockResolvedValue(undefined)
      const session = createFakeSession()
      host.sessions.set('client-1', session)
      host.scheduleTransportDisconnect('client-1', session)

      await vi.advanceTimersByTimeAsync(2_000)
      session.pc.connectionState = 'connected'
      session.pc.iceConnectionState = 'connected'
      host.noteTransportRestored('client-1', session, 'ice-restart')
      await vi.advanceTimersByTimeAsync(60_000)

      expect(session.transportReofferTimer).toBeUndefined()
      expect(connect).not.toHaveBeenCalled()
      expect(host.sessions.get('client-1')).toBe(session)
    })

    it('re-offer runs once per down episode', async () => {
      vi.useFakeTimers()
      const host = createHost()
      const connect = vi.spyOn(host, 'connectClientInner').mockResolvedValue(undefined)
      // The stale session stays in the map, so its own 10 s timer is still live.
      vi.spyOn(host, 'closeClientInner').mockResolvedValue(undefined)
      const session = createFakeSession()
      host.sessions.set('client-1', session)
      host.scheduleTransportDisconnect('client-1', session)
      host.scheduleTransportDisconnect('client-1', session)

      await vi.advanceTimersByTimeAsync(3_000)
      expect(connect).toHaveBeenCalledTimes(1)

      await vi.advanceTimersByTimeAsync(20_000)
      expect(connect).toHaveBeenCalledTimes(1)
    })

    it('transportReofferAfterMs: 0 disables the re-offer', async () => {
      vi.useFakeTimers()
      const host = createHost({ transportReofferAfterMs: 0 })
      const connect = vi.spyOn(host, 'connectClientInner').mockResolvedValue(undefined)
      const session = createFakeSession()
      host.sessions.set('client-1', session)
      host.scheduleTransportDisconnect('client-1', session)

      expect(vi.getTimerCount()).toBe(1)
      await vi.advanceTimersByTimeAsync(9_999)
      expect(host.sessions.has('client-1')).toBe(true)
      await vi.advanceTimersByTimeAsync(1)
      expect(host.sessions.has('client-1')).toBe(false)
      expect(connect).not.toHaveBeenCalled()
    })

    it('a replacement offered after transport loss gets the transport grace added to its connect-stall bound', async () => {
      vi.useFakeTimers()
      const logs: string[] = []
      const host = createHost({ logs })
      const { session, state, pc } = await connectRealPeer(host, 'client-1', { reoffer: true })
      state.conn = 'connecting'
      state.ice = 'connected'
      pc.oniceconnectionstatechange()

      await vi.advanceTimersByTimeAsync(19_999)
      expect(host.sessions.get('client-1')).toBe(session)

      await vi.advanceTimersByTimeAsync(1)
      expect(host.sessions.has('client-1')).toBe(false)
      expect(logs.some((l) => l.includes('after 20000ms reoffer=true — closing peer'))).toBe(true)
    })

    it('a normal connect keeps the 10 s connect-stall bound', async () => {
      vi.useFakeTimers()
      const host = createHost()
      const { session, state, pc } = await connectRealPeer(host, 'client-1')
      state.conn = 'connecting'
      state.ice = 'connected'
      pc.oniceconnectionstatechange()

      await vi.advanceTimersByTimeAsync(9_999)
      expect(host.sessions.get('client-1')).toBe(session)
      await vi.advanceTimersByTimeAsync(1)
      expect(host.sessions.has('client-1')).toBe(false)
    })

    it('one-way loss the client has not noticed: the server re-offers before the window ends', async () => {
      vi.useFakeTimers()
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
      const host = createHost({ voiceHandler: pod.wrapVoiceHandler('session-1') })
      pod.slots.set('session-1', { sessionId: 'session-1', host })
      const replacement = createFakeSession('connecting')
      // The replacement is connecting: it holds the slot and is not yet transport-ready.
      const connect = vi
        .spyOn(host, 'connectClientInner')
        .mockImplementation(async (peerId: string) => {
          host.sessions.set(peerId, replacement)
        })

      const stale = createFakeSession()
      host.sessions.set('client-1', stale)
      host.scheduleTransportDisconnect('client-1', stale)

      await vi.advanceTimersByTimeAsync(3_000)
      expect(connect).toHaveBeenCalledTimes(1)
      expect(connect).toHaveBeenCalledWith('client-1', { reoffer: true })
      expect(stale.pc.close).toHaveBeenCalledTimes(1)

      await vi.advanceTimersByTimeAsync(12_000)
      expect(host.sessions.get('client-1')).toBe(replacement)
      expect(teardown).not.toHaveBeenCalled()
    })
  })
})
