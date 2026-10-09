import { describe, expect, it, vi } from 'vitest'

import { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'

type FakeChannel = { onmessage: ((event: { data: unknown }) => void) | null | undefined }

type FakeSession = {
  pc: { connectionState: string; iceConnectionState: string; close: ReturnType<typeof vi.fn> }
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
  clientHangup?: boolean
}

type HostAccess = VoiceAgentSessionHost & {
  sessions: Map<string, FakeSession>
  enqueuePeerOp: <T>(peerId: string, op: () => Promise<T>) => Promise<T>
  installClientHangupInterceptor: (
    peerId: string,
    session: FakeSession,
    channel: FakeChannel,
  ) => void
}

function createFakeSession(): FakeSession {
  return {
    pc: { connectionState: 'connected', iceConnectionState: 'connected', close: vi.fn() },
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

function createHost(logs: string[]): {
  host: HostAccess
  signalingHandlers: Map<string, (...args: never[]) => void>
} {
  const signalingHandlers = new Map<string, (...args: never[]) => void>()
  const signaling = {
    room: 'test-room',
    on: vi.fn((event: string, handler: (...args: never[]) => void) => {
      signalingHandlers.set(event, handler)
    }),
    sendOffer: vi.fn(),
    sendIceCandidate: vi.fn(),
  }
  const host = new VoiceAgentSessionHost(signaling as never, [], {
    voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } } as never,
    sessionMode: 'data-only',
    log: (m) => logs.push(m),
    sessionBudget: {
      tryAcquire: () => 'lease-test',
      release: () => undefined,
      snapshot: () => ({ active: 0, max: 0, available: 0, rejectedTotal: 0 }),
    },
  })
  return { host: host as unknown as HostAccess, signalingHandlers }
}

const HANGUP = JSON.stringify({ type: 'client_hangup' })

/** Wires a hangup interceptor on a fake control channel and returns the channel. */
function wire(host: HostAccess, peerId: string, session: FakeSession): FakeChannel {
  const channel: FakeChannel = { onmessage: undefined }
  host.installClientHangupInterceptor(peerId, session, channel)
  return channel
}

describe('late client_hangup after a rejoin', () => {
  it("a client_hangup from the replaced session's channel does not close the replacement", async () => {
    const logs: string[] = []
    const { host } = createHost(logs)
    const sessionA = createFakeSession()
    const sessionB = createFakeSession()
    host.sessions.set('p1', sessionA)
    const channelA = wire(host, 'p1', sessionA)
    // Rejoin: session B replaces A.
    host.sessions.set('p1', sessionB)

    channelA.onmessage?.({ data: HANGUP })
    await host.enqueuePeerOp('p1', async () => undefined)

    expect(host.sessions.get('p1')).toBe(sessionB)
    expect(sessionB.pc.close).not.toHaveBeenCalled()
    expect(logs.some((m) => m.includes('close ignored — session already replaced'))).toBe(true)
  })

  it('a client_hangup on the live session still closes it', async () => {
    const { host } = createHost([])
    const sessionA = createFakeSession()
    const sessionB = createFakeSession()
    host.sessions.set('p1', sessionA)
    wire(host, 'p1', sessionA)
    host.sessions.set('p1', sessionB)
    const channelB = wire(host, 'p1', sessionB)

    channelB.onmessage?.({ data: HANGUP })
    await host.enqueuePeerOp('p1', async () => undefined)

    expect(host.sessions.get('p1')).toBeUndefined()
    expect(sessionB.pc.close).toHaveBeenCalled()
  })
})

describe('late peer-left after a rejoin', () => {
  it('a late peer-left after a rejoin', async () => {
    const logs: string[] = []
    const { host, signalingHandlers } = createHost(logs)
    const sessionA = createFakeSession()
    const sessionB = createFakeSession()
    host.sessions.set('p1', sessionA)
    // Rejoin: session B (transport connected) replaces A.
    host.sessions.set('p1', sessionB)

    signalingHandlers.get('peer-left')?.('p1' as never)
    await host.enqueuePeerOp('p1', async () => undefined)

    // Documents current behavior: the live replacement's transport is up, so the peer is kept.
    expect(host.sessions.get('p1')).toBe(sessionB)
    expect(sessionB.pc.close).not.toHaveBeenCalled()
    expect(logs.some((m) => m.includes('signaling left but transport is up — keeping peer'))).toBe(
      true,
    )
  })
})
