import { describe, expect, it, vi } from 'vitest'

import { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'

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
}

type HostAccess = VoiceAgentSessionHost & {
  sessions: Map<string, FakeSession>
  enqueuePeerOp: <T>(peerId: string, op: () => Promise<T>) => Promise<T>
  voidCloseClient: (peerId: string, expected: FakeSession) => void
  closeClientInner: (peerId: string) => Promise<unknown>
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

function createHost(logs: string[]): HostAccess {
  const signaling = {
    room: 'test-room',
    on: vi.fn(),
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
  return host as unknown as HostAccess
}

describe('stale peer close after rejoin', () => {
  it("a stale session's close queued during a rejoin does not close the replacement", async () => {
    const logs: string[] = []
    const host = createHost(logs)
    const stale = createFakeSession()
    const fresh = createFakeSession()
    host.sessions.set('client-1', stale)

    await host.enqueuePeerOp('client-1', async () => {
      host.voidCloseClient('client-1', stale)
      await host.closeClientInner('client-1')
      host.sessions.set('client-1', fresh)
    })
    await host.enqueuePeerOp('client-1', async () => undefined)

    expect(host.sessions.get('client-1')).toBe(fresh)
    expect(fresh.pc.close).not.toHaveBeenCalled()
    expect(stale.pc.close).toHaveBeenCalled()
    expect(logs.some((m) => m.includes('close ignored — session already replaced'))).toBe(true)
  })

  it('close for the live session still closes it', async () => {
    const host = createHost([])
    const s = createFakeSession()
    host.sessions.set('client-1', s)

    host.voidCloseClient('client-1', s)
    await host.enqueuePeerOp('client-1', async () => undefined)

    expect(host.sessions.get('client-1')).toBeUndefined()
    expect(s.pc.close).toHaveBeenCalled()
  })
})
