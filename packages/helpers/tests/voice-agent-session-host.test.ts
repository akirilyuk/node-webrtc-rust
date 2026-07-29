import { describe, expect, it, vi } from 'vitest'

import { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'
import type { VoiceSessionContext, VoiceSessionHandler } from '../src/voice-session-handler.js'

/** True when SDP negotiates an audio m-line (used by data-only mode tests). */
export function sdpHasAudioMedia(sdp: string): boolean {
  return /^m=audio/m.test(sdp)
}

type FakeControlChannel = {
  readyState: string
  send: ReturnType<typeof vi.fn>
}

type FakeSession = {
  pc: { connectionState: string; close: ReturnType<typeof vi.fn> }
  controlChannel: FakeControlChannel
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
  inboundPromise?: Promise<unknown>
}

type HostTestAccess = VoiceAgentSessionHost & {
  sessions: Map<string, FakeSession>
  sessionMode: 'voice' | 'data-only'
  maybeNotifyPeerLifecycle: (peerId: string, session: FakeSession) => void
  maybeStartAgentWhenTransportReady: (peerId: string, session: FakeSession) => void
  startAgentSession: (peerId: string, inboundPromise: Promise<unknown>) => Promise<void>
  closeClientInner: (peerId: string) => Promise<void>
  createSessionContext: (
    peerId: string,
    agent: FakeSession['agent'],
    controlChannel: FakeControlChannel,
    syncChannel?: undefined,
  ) => VoiceSessionContext
}

function createStubSignaling() {
  return {
    room: 'test-room',
    on: vi.fn(),
  }
}

function createHost(
  voiceHandler: VoiceSessionHandler,
  sessionMode: 'voice' | 'data-only' = 'voice',
): HostTestAccess {
  const host = new VoiceAgentSessionHost(createStubSignaling() as never, [], {
    voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } } as never,
    voiceHandler,
    sessionMode,
    sessionBudget: {
      tryAcquire: () => 'lease-test',
      release: () => undefined,
      snapshot: () => ({ active: 0, max: 0, available: 0, rejectedTotal: 0 }),
    },
  })
  return host as unknown as HostTestAccess
}

function createFakeSession(overrides: Partial<FakeSession> = {}): FakeSession {
  return {
    pc: { connectionState: 'connected', close: vi.fn() },
    controlChannel: {
      readyState: 'open',
      send: vi.fn(),
    },
    agent: {
      attach: vi.fn(async () => undefined),
      start: vi.fn(async () => undefined),
      stop: vi.fn(async () => undefined),
      sendTextToTTS: vi.fn(async () => undefined),
      on: vi.fn(() => () => undefined),
      speechEvents: async function* () {
        /* no events in unit tests */
      },
    },
    agentOut: {
      writeSample: vi.fn(async () => undefined),
    },
    budgetLease: 'lease-test',
    agentStarted: false,
    agentStartInProgress: false,
    peerTransportReadyNotified: false,
    peerConnectedNotified: false,
    peerSignalingJoined: true,
    remoteDescriptionSet: false,
    offerSent: true,
    pendingAnswer: null,
    pendingIce: [],
    ...overrides,
  }
}

describe('data-only session mode', () => {
  it('sdpHasAudioMedia detects audio m-lines', () => {
    expect(sdpHasAudioMedia('v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n')).toBe(true)
    expect(sdpHasAudioMedia('v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n')).toBe(
      false,
    )
  })
})

describe('VoiceAgentSessionHost peer lifecycle', () => {
  it('fires transport-ready once when control opens before inbound audio; connected waits for agent start', async () => {
    const order: string[] = []
    let connectedSpeakReached = false
    const host = createHost({
      onPeerTransportReady: () => {
        order.push('transport')
      },
      onPeerConnected: async (ctx) => {
        order.push('connected')
        await ctx.speak('ready')
        connectedSpeakReached = true
      },
      // Prefer speechEvents path over SDK forwardVoiceAgentSpeechToDataChannel with fakes.
      onSpeechEvent: () => undefined,
    })

    const session = createFakeSession({
      pc: { connectionState: 'connected', close: vi.fn() },
      controlChannel: { readyState: 'open', send: vi.fn() },
    })
    host.sessions.set('client-1', session)

    // Control + PC ready before mic/inbound attach completes.
    host.maybeNotifyPeerLifecycle('client-1', session)
    host.maybeNotifyPeerLifecycle('client-1', session)

    expect(order).toEqual(['transport'])
    expect(session.peerTransportReadyNotified).toBe(true)
    expect(session.peerConnectedNotified).toBe(false)
    expect(connectedSpeakReached).toBe(false)

    let resolveInbound!: (track: unknown) => void
    const inboundPromise = new Promise((resolve) => {
      resolveInbound = resolve
    })

    const startPromise = host.startAgentSession('client-1', inboundPromise)
    // Still waiting on inbound — customer connected must not run.
    host.maybeNotifyPeerLifecycle('client-1', session)
    expect(order).toEqual(['transport'])
    expect(connectedSpeakReached).toBe(false)

    resolveInbound({ kind: 'audio' })
    await startPromise

    expect(order).toEqual(['transport', 'connected'])
    expect(session.agentStarted).toBe(true)
    expect(session.agentStartInProgress).toBe(false)
    expect(session.peerConnectedNotified).toBe(true)
    expect(connectedSpeakReached).toBe(true)
    expect(session.agent?.attach).toHaveBeenCalledTimes(1)
    expect(session.agent?.start).toHaveBeenCalledTimes(1)

    host.maybeNotifyPeerLifecycle('client-1', session)
    expect(order).toEqual(['transport', 'connected'])
  })

  it('data-only emits transport then connected once without VoiceAgent', () => {
    const order: string[] = []
    const host = createHost(
      {
        onPeerTransportReady: () => {
          order.push('transport')
        },
        onPeerConnected: () => {
          order.push('connected')
        },
      },
      'data-only',
    )

    const session = createFakeSession({
      agent: undefined,
      agentOut: undefined,
    })
    host.sessions.set('client-data', session)

    host.maybeNotifyPeerLifecycle('client-data', session)
    host.maybeNotifyPeerLifecycle('client-data', session)

    expect(order).toEqual(['transport', 'connected'])
    expect(session.peerTransportReadyNotified).toBe(true)
    expect(session.peerConnectedNotified).toBe(true)
  })

  it('does not let a synchronous transport hook failure block data-only connected', () => {
    const connected = vi.fn()
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    const host = createHost(
      {
        onPeerTransportReady: () => {
          throw new Error('transport hook failed')
        },
        onPeerConnected: connected,
      },
      'data-only',
    )
    const session = createFakeSession({
      agent: undefined,
      agentOut: undefined,
    })
    host.sessions.set('client-data-hook-fail', session)

    host.maybeNotifyPeerLifecycle('client-data-hook-fail', session)

    expect(connected).toHaveBeenCalledTimes(1)
    expect(session.peerTransportReadyNotified).toBe(true)
    expect(session.peerConnectedNotified).toBe(true)
    expect(consoleError).toHaveBeenCalledWith(
      '[session client-data-hook-fail] voiceHandler.onPeerTransportReady failed:',
      expect.any(Error),
    )
    consoleError.mockRestore()
  })

  it('disconnect between transport-ready and agent-ready uses onPeerDisconnected', async () => {
    const disconnected: string[] = []
    const signalingLost: string[] = []
    const host = createHost({
      onPeerTransportReady: () => undefined,
      onPeerDisconnected: (ctx) => {
        disconnected.push(ctx.peerId)
      },
      onPeerSignalingLost: (ctx) => {
        signalingLost.push(ctx.peerId)
      },
    })

    const session = createFakeSession()
    host.sessions.set('client-mid', session)
    host.maybeNotifyPeerLifecycle('client-mid', session)
    expect(session.peerTransportReadyNotified).toBe(true)
    expect(session.peerConnectedNotified).toBe(false)

    await host.disconnectPeer('client-mid')

    expect(disconnected).toEqual(['client-mid'])
    expect(signalingLost).toEqual([])
  })

  it('pre-transport teardown uses onPeerSignalingLost', async () => {
    const disconnected: string[] = []
    const signalingLost: string[] = []
    const host = createHost({
      onPeerDisconnected: (ctx) => {
        disconnected.push(ctx.peerId)
      },
      onPeerSignalingLost: (ctx) => {
        signalingLost.push(ctx.peerId)
      },
    })

    const session = createFakeSession({
      controlChannel: { readyState: 'connecting', send: vi.fn() },
      pc: { connectionState: 'connecting', close: vi.fn() },
    })
    host.sessions.set('client-early', session)
    host.maybeNotifyPeerLifecycle('client-early', session)
    expect(session.peerTransportReadyNotified).toBe(false)

    await host.disconnectPeer('client-early')

    expect(signalingLost).toEqual(['client-early'])
    expect(disconnected).toEqual([])
  })

  it('failed agent start does not advertise onPeerConnected and allows retry', async () => {
    const order: string[] = []
    const host = createHost({
      onPeerTransportReady: () => {
        order.push('transport')
      },
      onPeerConnected: () => {
        order.push('connected')
      },
      onSpeechEvent: () => undefined,
    })

    const session = createFakeSession({
      agent: {
        attach: vi.fn(async () => {
          throw new Error('attach failed')
        }),
        start: vi.fn(async () => undefined),
        stop: vi.fn(async () => undefined),
        sendTextToTTS: vi.fn(async () => undefined),
        on: vi.fn(() => () => undefined),
        speechEvents: async function* () {},
      },
    })
    host.sessions.set('client-fail', session)
    host.maybeNotifyPeerLifecycle('client-fail', session)
    expect(order).toEqual(['transport'])

    await expect(
      host.startAgentSession('client-fail', Promise.resolve({ kind: 'audio' })),
    ).rejects.toThrow('attach failed')

    expect(session.agentStarted).toBe(false)
    expect(session.agentStartInProgress).toBe(false)
    expect(session.peerConnectedNotified).toBe(false)
    expect(order).toEqual(['transport'])

    // Retry after failure can succeed.
    session.agent!.attach = vi.fn(async () => undefined)
    await host.startAgentSession('client-fail', Promise.resolve({ kind: 'audio' }))
    expect(order).toEqual(['transport', 'connected'])
    expect(session.agentStarted).toBe(true)
    expect(session.agentStartInProgress).toBe(false)
  })

  it('stops a started agent when post-start setup fails', async () => {
    const connected = vi.fn()
    const host = createHost({
      onPeerConnected: connected,
      onSpeechEvent: () => undefined,
    })
    const session = createFakeSession({
      agentOut: {
        writeSample: vi.fn(async () => {
          throw new Error('kick failed')
        }),
      },
    })
    host.sessions.set('client-kick-fail', session)

    await expect(
      host.startAgentSession('client-kick-fail', Promise.resolve({ kind: 'audio' })),
    ).rejects.toThrow('kick failed')

    expect(session.agent?.stop).toHaveBeenCalledTimes(1)
    expect(session.agentStarted).toBe(false)
    expect(session.agentStartInProgress).toBe(false)
    expect(session.peerConnectedNotified).toBe(false)
    expect(connected).not.toHaveBeenCalled()
  })
})

describe('VoiceAgentSessionHost deferred agent start', () => {
  it('does not start VoiceAgent when PC is connected but control DC is not open yet', () => {
    const host = createHost({ onSpeechEvent: () => undefined })
    const session = createFakeSession({
      pc: { connectionState: 'connected', close: vi.fn() },
      controlChannel: { readyState: 'connecting', send: vi.fn() },
    })
    session.inboundPromise = Promise.resolve({ kind: 'audio' })
    host.sessions.set('client-dc-wait', session)

    host.maybeStartAgentWhenTransportReady('client-dc-wait', session)

    expect(session.agent?.start).not.toHaveBeenCalled()
    expect(session.agent?.attach).not.toHaveBeenCalled()
    expect(session.inboundPromise).toBeDefined()
  })

  it('starts VoiceAgent when control DC opens after PC is already connected', async () => {
    const host = createHost({ onSpeechEvent: () => undefined })
    const session = createFakeSession({
      pc: { connectionState: 'connected', close: vi.fn() },
      controlChannel: { readyState: 'connecting', send: vi.fn() },
    })
    let resolveInbound!: (track: unknown) => void
    const inboundPromise = new Promise((resolve) => {
      resolveInbound = resolve
    })
    session.inboundPromise = inboundPromise
    host.sessions.set('client-dc-late', session)

    host.maybeStartAgentWhenTransportReady('client-dc-late', session)
    expect(session.agent?.start).not.toHaveBeenCalled()
    expect(session.inboundPromise).toBe(inboundPromise)

    session.controlChannel.readyState = 'open'
    host.maybeStartAgentWhenTransportReady('client-dc-late', session)
    expect(session.inboundPromise).toBeUndefined()

    resolveInbound({ kind: 'audio' })
    await vi.waitFor(() => {
      expect(session.agent?.start).toHaveBeenCalledTimes(1)
    })
    expect(session.agent?.attach).toHaveBeenCalledTimes(1)
  })

  it('starts VoiceAgent when control DC is already open when PC becomes connected', async () => {
    const host = createHost({ onSpeechEvent: () => undefined })
    const session = createFakeSession({
      pc: { connectionState: 'connecting', close: vi.fn() },
      controlChannel: { readyState: 'open', send: vi.fn() },
    })
    let resolveInbound!: (track: unknown) => void
    const inboundPromise = new Promise((resolve) => {
      resolveInbound = resolve
    })
    session.inboundPromise = inboundPromise
    host.sessions.set('client-pc-late', session)

    host.maybeStartAgentWhenTransportReady('client-pc-late', session)
    expect(session.agent?.start).not.toHaveBeenCalled()
    expect(session.inboundPromise).toBe(inboundPromise)

    session.pc.connectionState = 'connected'
    host.maybeStartAgentWhenTransportReady('client-pc-late', session)
    expect(session.inboundPromise).toBeUndefined()

    resolveInbound({ kind: 'audio' })
    await vi.waitFor(() => {
      expect(session.agent?.start).toHaveBeenCalledTimes(1)
    })
    expect(session.agent?.attach).toHaveBeenCalledTimes(1)
  })
})
