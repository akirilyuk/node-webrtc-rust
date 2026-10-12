import { EventEmitter } from 'node:events'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { SignalingServer } from '@node-webrtc-rust/signaling'

import { SessionPod } from '../src/session-pod.js'
import {
  CONNECT_STALL_TIMEOUT_MS,
  type ConnectStallFields,
  VoiceAgentSessionHost,
} from '../src/voice-agent-session-host.js'

/**
 * Connect-stall watchdog (error type `net.connect-stall-no-timeout`): ICE connected but the
 * connection never reached `connected` (stalled DTLS handshake; the vendored DTLS has no timeout).
 * The host must close the peer through its normal close path so the slot is released.
 */

type FakeSession = {
  pc: {
    connectionState: string
    iceConnectionState: string
    close: ReturnType<typeof vi.fn>
    closeAsync: ReturnType<typeof vi.fn>
  }
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
  connectStallTimer?: ReturnType<typeof setTimeout>
}

type HostAccess = VoiceAgentSessionHost & {
  sessions: Map<string, FakeSession>
  armConnectStallTimer: (peerId: string, session: FakeSession) => void
  clearConnectStallTimer: (session: FakeSession) => void
  closeClientInner: (peerId: string) => Promise<unknown>
}

class FakeSignaling extends EventEmitter {
  room = 'test-room'
  sendIceCandidate = vi.fn()
  sendOffer = vi.fn()
}

function createFakeSession(): FakeSession {
  return {
    pc: {
      connectionState: 'connecting',
      iceConnectionState: 'connected',
      close: vi.fn(),
      closeAsync: vi.fn(async () => undefined),
    },
    controlChannel: { readyState: 'open', send: vi.fn() },
    budgetLease: 'lease-1',
    agentStarted: false,
    agentStartInProgress: false,
    peerTransportReadyNotified: false,
    peerConnectedNotified: false,
    peerSignalingJoined: false,
    remoteDescriptionSet: true,
    offerSent: true,
    pendingAnswer: null,
    pendingIce: [],
  }
}

function setup(options: { connectStallTimeoutMs?: number } = {}) {
  const logs: string[] = []
  const stalls: ConnectStallFields[] = []
  const release = vi.fn()
  const host = new VoiceAgentSessionHost(new FakeSignaling() as never, [], {
    voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } } as never,
    log: (m) => logs.push(m),
    onConnectStall: (f) => stalls.push(f),
    sessionBudget: {
      tryAcquire: () => 'lease-1',
      release,
      snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
    } as never,
    ...options,
  }) as unknown as HostAccess
  const session = createFakeSession()
  host.sessions.set('client-1', session)
  return { host, session, logs, stalls, release }
}

describe('connect-stall watchdog', () => {
  afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('a peer stuck in connecting after ICE connected is closed after connectStallTimeoutMs', async () => {
    vi.useFakeTimers()
    const { host, session, logs, stalls, release } = setup()
    host.armConnectStallTimer('client-1', session)

    await vi.advanceTimersByTimeAsync(CONNECT_STALL_TIMEOUT_MS - 1)
    expect(CONNECT_STALL_TIMEOUT_MS).toBe(10_000)
    expect(session.pc.closeAsync).not.toHaveBeenCalled()
    expect(stalls).toEqual([])
    expect(host.sessions.has('client-1')).toBe(true)

    await vi.advanceTimersByTimeAsync(1)
    await vi.waitFor(() => expect(release).toHaveBeenCalledWith('lease-1'))
    expect(session.pc.closeAsync).toHaveBeenCalledTimes(1)
    expect(host.sessions.has('client-1')).toBe(false)
    expect(logs).toContain(
      '[voice client-1] connect stalled: ice=connected connectionState=connecting after 10000ms — closing peer',
    )
    expect(stalls).toEqual([
      { peerId: 'client-1', iceState: 'connected', connectionState: 'connecting', afterMs: 10_000 },
    ])
  })

  it('a peer that reaches connected in time is not closed', async () => {
    vi.useFakeTimers()
    const { host, session, stalls } = setup()
    host.armConnectStallTimer('client-1', session)

    await vi.advanceTimersByTimeAsync(9_900)
    session.pc.connectionState = 'connected'
    host.clearConnectStallTimer(session)
    await vi.advanceTimersByTimeAsync(20_000)

    expect(session.pc.closeAsync).not.toHaveBeenCalled()
    expect(host.sessions.get('client-1')).toBe(session)
    expect(stalls).toEqual([])
    expect(vi.getTimerCount()).toBe(0)
  })

  it('does not arm when the connection is already connected', async () => {
    vi.useFakeTimers()
    const { host, session } = setup()
    session.pc.connectionState = 'connected'
    host.armConnectStallTimer('client-1', session)
    expect(vi.getTimerCount()).toBe(0)
  })

  it('connectStallTimeoutMs 0 disables the watchdog', async () => {
    vi.useFakeTimers()
    const { host, session, stalls } = setup({ connectStallTimeoutMs: 0 })
    host.armConnectStallTimer('client-1', session)
    expect(vi.getTimerCount()).toBe(0)
    await vi.advanceTimersByTimeAsync(60_000)
    expect(session.pc.closeAsync).not.toHaveBeenCalled()
    expect(stalls).toEqual([])
  })

  it('arms one timer per peer however often ICE reports connected', async () => {
    vi.useFakeTimers()
    const { host, session } = setup()
    host.armConnectStallTimer('client-1', session)
    host.armConnectStallTimer('client-1', session)
    expect(vi.getTimerCount()).toBe(1)
  })

  it('honours a custom connectStallTimeoutMs', async () => {
    vi.useFakeTimers()
    const { host, session, stalls } = setup({ connectStallTimeoutMs: 2_000 })
    host.armConnectStallTimer('client-1', session)
    await vi.advanceTimersByTimeAsync(2_000)
    expect(stalls).toEqual([
      { peerId: 'client-1', iceState: 'connected', connectionState: 'connecting', afterMs: 2_000 },
    ])
  })

  it('teardown before expiry clears the timer (no late callback or double close)', async () => {
    vi.useFakeTimers()
    const { host, session, stalls, logs } = setup()
    host.armConnectStallTimer('client-1', session)
    expect(vi.getTimerCount()).toBe(1)

    await host.closeClientInner('client-1')
    expect(session.connectStallTimer).toBeUndefined()
    expect(vi.getTimerCount()).toBe(0)
    expect(session.pc.closeAsync).toHaveBeenCalledTimes(1)

    await vi.advanceTimersByTimeAsync(60_000)
    expect(session.pc.closeAsync).toHaveBeenCalledTimes(1)
    expect(stalls).toEqual([])
    expect(logs.some((l) => l.includes('connect stalled'))).toBe(false)
  })
})

describe('SessionPod passes the connect-stall options to every host', () => {
  let server: SignalingServer | undefined

  afterEach(async () => {
    if (server) {
      await server.close().catch(() => undefined)
      server = undefined
    }
  })

  it('connectStallTimeoutMs and onConnectStall reach the slot host', async () => {
    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0, '127.0.0.1')
    const onConnectStall = vi.fn()
    const pod = new SessionPod(server, {
      signalingUrl: `ws://127.0.0.1:${server.port}/ws`,
      iceServers: [],
      voiceConfig: {} as never,
      sessionMode: 'data-only',
      connectStallTimeoutMs: 1234,
      onConnectStall,
    })
    await pod.ensureSession('session-a')
    const slots = (
      pod as unknown as {
        slots: Map<
          string,
          {
            host: {
              connectStallTimeoutMs: number
              options: { onConnectStall?: unknown }
            }
          }
        >
      }
    ).slots
    const host = slots.get('session-a')?.host
    expect(host?.connectStallTimeoutMs).toBe(1234)
    expect(host?.options.onConnectStall).toBe(onConnectStall)
    await pod.close().catch(() => undefined)
    server = undefined
  })
})
