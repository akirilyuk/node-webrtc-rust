import { EventEmitter } from 'node:events'
import { describe, expect, it, vi } from 'vitest'

import { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'
import { VoiceSessionBudget } from '../src/voice-session-budget.js'
import type { VoiceSessionHandler } from '../src/voice-session-handler.js'

type FakeSession = {
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

describe('VoiceAgentSessionHost peer close / budget release', () => {
  it('releases session budget only after closeAsync completes', async () => {
    const release = vi.fn()
    const host = createHost(
      {},
      {
        tryAcquire: () => 'lease-7',
        release,
        snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
      },
    )

    let resolveClose!: () => void
    const closeGate = new Promise<void>((resolve) => {
      resolveClose = resolve
    })
    const closeAsync = vi.fn(() => closeGate)
    host.sessions.set(
      'client-1',
      createFakeSession({
        budgetLease: 'lease-7',
        pc: { connectionState: 'connected', close: vi.fn(), closeAsync },
      }),
    )

    const closing = host.disconnectPeer('client-1')
    await vi.waitFor(() => {
      expect(host.sessions.has('client-1')).toBe(false)
    })
    expect(release).not.toHaveBeenCalled()

    resolveClose()
    await closing
    expect(closeAsync).toHaveBeenCalledTimes(1)
    expect(release).toHaveBeenCalledWith('lease-7')
  })

  it('quarantines and does not release when closeAsync rejects', async () => {
    const release = vi.fn()
    const host = createHost(
      {},
      {
        tryAcquire: () => 'lease-3',
        release,
        snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
      },
    )
    const closeAsync = vi.fn(async () => {
      throw new Error('native close failed')
    })
    host.sessions.set(
      'client-1',
      createFakeSession({
        budgetLease: 'lease-3',
        pc: { connectionState: 'connected', close: vi.fn(), closeAsync },
      }),
    )

    const outcome = await host.disconnectPeer('client-1')
    expect(outcome).toMatchObject({
      status: 'failed',
      quarantined: true,
      pc: 'failed',
      agent: 'ok',
    })
    expect(release).not.toHaveBeenCalled()
    expect(host.isRecycleRequired).toBe(true)
    expect(host.quarantinedCount).toBe(1)
    expect(host.sessionBudgetSnapshot.recycleRequired).toBe(true)
  })

  it('serializes concurrent disconnectPeer calls to one closeAsync', async () => {
    const release = vi.fn()
    const host = createHost(
      {},
      {
        tryAcquire: () => 'lease-1',
        release,
        snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
      },
    )

    let resolveClose!: () => void
    const closeGate = new Promise<void>((resolve) => {
      resolveClose = resolve
    })
    const closeAsync = vi.fn(() => closeGate)
    host.sessions.set(
      'client-1',
      createFakeSession({
        budgetLease: 'lease-1',
        pc: { connectionState: 'connected', close: vi.fn(), closeAsync },
      }),
    )

    const a = host.disconnectPeer('client-1')
    const b = host.disconnectPeer('client-1')
    resolveClose()
    await Promise.all([a, b])
    expect(closeAsync).toHaveBeenCalledTimes(1)
    expect(release).toHaveBeenCalledTimes(1)
  })

  it('lifecycle disconnect hooks are nonblocking for budget release', async () => {
    const release = vi.fn()
    let hookStarted = false
    let resolveHook!: () => void
    const hookGate = new Promise<void>((resolve) => {
      resolveHook = resolve
    })
    const host = createHost(
      {
        onPeerDisconnected: async () => {
          hookStarted = true
          await hookGate
        },
      },
      {
        tryAcquire: () => 'lease-hook',
        release,
        snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
      },
    )
    host.sessions.set(
      'client-hook',
      createFakeSession({
        budgetLease: 'lease-hook',
        pc: {
          connectionState: 'connected',
          close: vi.fn(),
          closeAsync: vi.fn(async () => undefined),
        },
      }),
    )

    await host.disconnectPeer('client-hook')
    expect(hookStarted).toBe(true)
    expect(release).toHaveBeenCalledWith('lease-hook')
    resolveHook()
  })

  it('stale close does not release the replacement opaque lease', async () => {
    const budget = new VoiceSessionBudget(1)
    const host = createHost({}, budget)

    let resolveClose!: () => void
    const closeGate = new Promise<void>((resolve) => {
      resolveClose = resolve
    })
    const closeAsync = vi.fn(() => closeGate)
    const lease1 = budget.acquire('client-rejoin')
    host.sessions.set(
      'client-rejoin',
      createFakeSession({
        budgetLease: lease1,
        pc: { connectionState: 'connected', close: vi.fn(), closeAsync },
      }),
    )

    const closing = host.disconnectPeer('client-rejoin')
    await vi.waitFor(() => {
      expect(host.sessions.has('client-rejoin')).toBe(false)
    })
    expect(budget.snapshot().active).toBe(1)

    // Replacement acquire while old close still in flight — capacity still held by lease1.
    expect(budget.tryAcquire('client-rejoin')).toBeNull()
    // Manually release is not done; force a second lease only after we free capacity incorrectly
    // would be wrong. Instead: wait until close finishes, then new lease.
    resolveClose()
    await closing
    expect(budget.snapshot().active).toBe(0)

    const lease2 = budget.acquire('client-rejoin')
    expect(lease2).not.toBe(lease1)
    host.sessions.set(
      'client-rejoin',
      createFakeSession({
        budgetLease: lease2,
        pc: {
          connectionState: 'connected',
          close: vi.fn(),
          closeAsync: vi.fn(async () => undefined),
        },
      }),
    )
    expect(budget.snapshot().active).toBe(1)
    await host.disconnectPeer('client-rejoin')
    expect(budget.snapshot().active).toBe(0)
  })

  it('host close awaits closing peers and rejects new connects', async () => {
    const release = vi.fn()
    const host = createHost(
      {},
      {
        tryAcquire: () => 'lease-1',
        release,
        snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
      },
    )

    let resolveClose!: () => void
    const closeGate = new Promise<void>((resolve) => {
      resolveClose = resolve
    })
    host.sessions.set(
      'client-slow',
      createFakeSession({
        budgetLease: 'lease-1',
        pc: {
          connectionState: 'connected',
          close: vi.fn(),
          closeAsync: vi.fn(() => closeGate),
        },
      }),
    )

    const closing = host.close()
    expect(host.hostClosing).toBe(true)
    expect(host.activeClientCount).toBe(1)
    resolveClose()
    await closing
    expect(host.activeClientCount).toBe(0)
    expect(release).toHaveBeenCalledWith('lease-1')
  })

  it('awaits VoiceAgent.stop under the same teardown path', async () => {
    const release = vi.fn()
    const host = createHost(
      {},
      {
        tryAcquire: () => 'lease-agent',
        release,
        snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
      },
    )
    let resolveStop!: () => void
    const stopGate = new Promise<void>((resolve) => {
      resolveStop = resolve
    })
    const stop = vi.fn(() => stopGate)
    const closeAsync = vi.fn(async () => undefined)
    host.sessions.set(
      'client-agent',
      createFakeSession({
        budgetLease: 'lease-agent',
        agent: {
          attach: vi.fn(async () => undefined),
          start: vi.fn(async () => undefined),
          stop,
          sendTextToTTS: vi.fn(async () => undefined),
          on: vi.fn(() => () => undefined),
          speechEvents: async function* () {
            /* empty */
          },
        },
        pc: { connectionState: 'connected', close: vi.fn(), closeAsync },
      }),
    )

    const closing = host.disconnectPeer('client-agent')
    expect(release).not.toHaveBeenCalled()
    resolveStop()
    await closing
    expect(stop).toHaveBeenCalledTimes(1)
    expect(closeAsync).toHaveBeenCalledTimes(1)
    expect(release).toHaveBeenCalledWith('lease-agent')
  })

  it('releases capacity without quarantine when pre-transport teardown times out', async () => {
    vi.useFakeTimers()
    try {
      const release = vi.fn()
      const host = createHost(
        {},
        {
          tryAcquire: () => 'lease-pre-transport',
          release,
          snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
        },
      )

      const closeAsync = vi.fn(() => new Promise<void>(() => undefined))
      host.sessions.set(
        'client-pre-transport',
        createFakeSession({
          budgetLease: 'lease-pre-transport',
          peerTransportReadyNotified: false,
          peerConnectedNotified: false,
          pc: { connectionState: 'connected', close: vi.fn(), closeAsync },
        }),
      )

      const closing = host.disconnectPeer('client-pre-transport')
      await vi.advanceTimersByTimeAsync(5_000)
      const outcome = await closing
      expect(outcome).toEqual({
        status: 'timed_out',
        pc: 'timed_out',
        agent: 'ok',
      })
      expect(release).toHaveBeenCalledWith('lease-pre-transport')
      expect(host.isRecycleRequired).toBe(false)
      expect(host.quarantinedCount).toBe(0)
    } finally {
      vi.useRealTimers()
    }
  })

  it('quarantines after bounded timeout when closeAsync hangs (no placement)', async () => {
    vi.useFakeTimers()
    try {
      const release = vi.fn()
      const tryAcquire = vi.fn(() => null as string | null)
      const host = createHost(
        {},
        {
          tryAcquire,
          release,
          snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
        },
      )

      const closeAsync = vi.fn(() => new Promise<void>(() => undefined))
      host.sessions.set(
        'client-slow',
        createFakeSession({
          budgetLease: 'lease-slow',
          pc: { connectionState: 'connected', close: vi.fn(), closeAsync },
        }),
      )

      const closing = host.disconnectPeer('client-slow')
      await vi.advanceTimersByTimeAsync(5_000)
      const outcome = await closing
      expect(outcome).toEqual({
        status: 'timed_out',
        quarantined: true,
        pc: 'timed_out',
        agent: 'ok',
      })
      expect(release).not.toHaveBeenCalled()
      expect(host.isRecycleRequired).toBe(true)
      expect(host.quarantinedCount).toBe(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('quarantines and does not release when VoiceAgent.stop rejects', async () => {
    const release = vi.fn()
    const host = createHost(
      {},
      {
        tryAcquire: () => 'lease-stop-fail',
        release,
        snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
      },
    )
    const stop = vi.fn(async () => {
      throw new Error('TTS shutdown unhealthy')
    })
    host.sessions.set(
      'client-stop-fail',
      createFakeSession({
        budgetLease: 'lease-stop-fail',
        agent: {
          attach: vi.fn(async () => undefined),
          start: vi.fn(async () => undefined),
          stop,
          sendTextToTTS: vi.fn(async () => undefined),
          on: vi.fn(() => () => undefined),
          speechEvents: async function* () {
            /* empty */
          },
        },
        pc: {
          connectionState: 'connected',
          close: vi.fn(),
          closeAsync: vi.fn(async () => undefined),
        },
      }),
    )

    const outcome = await host.disconnectPeer('client-stop-fail')
    expect(outcome).toMatchObject({
      status: 'failed',
      quarantined: true,
      pc: 'ok',
      agent: 'failed',
    })
    expect(release).not.toHaveBeenCalled()
    expect(host.isRecycleRequired).toBe(true)
  })

  it('quarantines and does not release when VoiceAgent.stop hangs', async () => {
    vi.useFakeTimers()
    try {
      const release = vi.fn()
      const host = createHost(
        {},
        {
          tryAcquire: () => 'lease-stop-hang',
          release,
          snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
        },
      )
      const stop = vi.fn(() => new Promise<void>(() => undefined))
      host.sessions.set(
        'client-stop-hang',
        createFakeSession({
          budgetLease: 'lease-stop-hang',
          agent: {
            attach: vi.fn(async () => undefined),
            start: vi.fn(async () => undefined),
            stop,
            sendTextToTTS: vi.fn(async () => undefined),
            on: vi.fn(() => () => undefined),
            speechEvents: async function* () {
              /* empty */
            },
          },
          pc: {
            connectionState: 'connected',
            close: vi.fn(),
            closeAsync: vi.fn(async () => undefined),
          },
        }),
      )

      const closing = host.disconnectPeer('client-stop-hang')
      await vi.advanceTimersByTimeAsync(5_000)
      const outcome = await closing
      expect(outcome).toEqual({
        status: 'timed_out',
        quarantined: true,
        pc: 'ok',
        agent: 'timed_out',
      })
      expect(release).not.toHaveBeenCalled()
      expect(host.isRecycleRequired).toBe(true)
    } finally {
      vi.useRealTimers()
    }
  })

  it('late native close clears quarantine exactly once when safe', async () => {
    vi.useFakeTimers()
    try {
      const budget = new VoiceSessionBudget(1)
      const host = createHost({}, budget)

      let resolveClose!: () => void
      const closeGate = new Promise<void>((resolve) => {
        resolveClose = resolve
      })
      const lease = budget.acquire('client-late')
      host.sessions.set(
        'client-late',
        createFakeSession({
          budgetLease: lease,
          pc: {
            connectionState: 'connected',
            close: vi.fn(),
            closeAsync: vi.fn(() => closeGate),
          },
        }),
      )

      const closing = host.disconnectPeer('client-late')
      await vi.advanceTimersByTimeAsync(5_000)
      const outcome = await closing
      expect(outcome).toEqual({
        status: 'timed_out',
        quarantined: true,
        pc: 'timed_out',
        agent: 'ok',
      })
      expect(budget.snapshot().active).toBe(1)
      expect(host.isRecycleRequired).toBe(true)

      resolveClose()
      await vi.waitFor(() => {
        expect(host.quarantinedCount).toBe(0)
      })
      expect(budget.snapshot().active).toBe(0)
      expect(host.isRecycleRequired).toBe(false)

      // Exactly-once: a second budget.release of the same lease is a no-op.
      budget.release(lease)
      expect(budget.snapshot().active).toBe(0)
    } finally {
      vi.useRealTimers()
    }
  })

  it('late PC close alone does not clear when agent stop is still pending', async () => {
    vi.useFakeTimers()
    try {
      const budget = new VoiceSessionBudget(1)
      const host = createHost({}, budget)
      let resolveClose!: () => void
      const closeGate = new Promise<void>((resolve) => {
        resolveClose = resolve
      })
      const stop = vi.fn(() => new Promise<void>(() => undefined))
      const lease = budget.acquire('client-both-pending')
      host.sessions.set(
        'client-both-pending',
        createFakeSession({
          budgetLease: lease,
          agent: {
            attach: vi.fn(async () => undefined),
            start: vi.fn(async () => undefined),
            stop,
            sendTextToTTS: vi.fn(async () => undefined),
            on: vi.fn(() => () => undefined),
            speechEvents: async function* () {
              /* empty */
            },
          },
          pc: {
            connectionState: 'connected',
            close: vi.fn(),
            closeAsync: vi.fn(() => closeGate),
          },
        }),
      )

      const closing = host.disconnectPeer('client-both-pending')
      await vi.advanceTimersByTimeAsync(5_000)
      await closing
      expect(host.isRecycleRequired).toBe(true)
      expect(budget.snapshot().active).toBe(1)

      resolveClose()
      await Promise.resolve()
      await Promise.resolve()
      expect(host.quarantinedCount).toBe(1)
      expect(budget.snapshot().active).toBe(1)
      expect(host.isRecycleRequired).toBe(true)
    } finally {
      vi.useRealTimers()
    }
  })

  it('late agent stop clears quarantine only when PC close also confirmed', async () => {
    vi.useFakeTimers()
    try {
      const budget = new VoiceSessionBudget(1)
      const host = createHost({}, budget)
      let resolveStop!: () => void
      const stopGate = new Promise<void>((resolve) => {
        resolveStop = resolve
      })
      const lease = budget.acquire('client-late-agent')
      host.sessions.set(
        'client-late-agent',
        createFakeSession({
          budgetLease: lease,
          agent: {
            attach: vi.fn(async () => undefined),
            start: vi.fn(async () => undefined),
            stop: vi.fn(() => stopGate),
            sendTextToTTS: vi.fn(async () => undefined),
            on: vi.fn(() => () => undefined),
            speechEvents: async function* () {
              /* empty */
            },
          },
          pc: {
            connectionState: 'connected',
            close: vi.fn(),
            closeAsync: vi.fn(async () => undefined),
          },
        }),
      )

      const closing = host.disconnectPeer('client-late-agent')
      await vi.advanceTimersByTimeAsync(5_000)
      const outcome = await closing
      expect(outcome).toEqual({
        status: 'timed_out',
        quarantined: true,
        pc: 'ok',
        agent: 'timed_out',
      })
      expect(budget.snapshot().active).toBe(1)

      resolveStop()
      await vi.waitFor(() => {
        expect(host.quarantinedCount).toBe(0)
      })
      expect(budget.snapshot().active).toBe(0)
      expect(host.isRecycleRequired).toBe(false)
    } finally {
      vi.useRealTimers()
    }
  })

  it('overlapping teardown cannot double-release a slot', async () => {
    const budget = new VoiceSessionBudget(1)
    const host = createHost({}, budget)
    const lease = budget.acquire('client-once')
    host.sessions.set(
      'client-once',
      createFakeSession({
        budgetLease: lease,
        pc: {
          connectionState: 'connected',
          close: vi.fn(),
          closeAsync: vi.fn(async () => undefined),
        },
      }),
    )

    const [a, b] = await Promise.all([
      host.disconnectPeer('client-once'),
      host.disconnectPeer('client-once'),
    ])
    // Per-peer FIFO: first teardown closes + releases; second observes absent.
    expect([a.status, b.status].sort()).toEqual(['absent', 'closed'])
    expect(budget.snapshot().active).toBe(0)
    // Manual second release is a no-op on the budget.
    budget.release(lease)
    expect(budget.snapshot().active).toBe(0)
  })

  it('falls back to sync close when closeAsync is unavailable', async () => {
    const release = vi.fn()
    const host = createHost(
      {},
      {
        tryAcquire: () => 'lease-sync',
        release,
        snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
      },
    )
    const close = vi.fn()
    host.sessions.set(
      'client-sync',
      createFakeSession({
        budgetLease: 'lease-sync',
        pc: { connectionState: 'connected', close },
      }),
    )

    await host.disconnectPeer('client-sync')
    expect(close).toHaveBeenCalledTimes(1)
    expect(release).toHaveBeenCalledWith('lease-sync')
  })

  it('peer-left during gated connect closes the resulting generation', async () => {
    const signaling = new FakeSignaling()
    const budget = new VoiceSessionBudget(2)
    const host = createHost({}, budget, signaling)

    let resolveConnectGate!: () => void
    const connectGate = new Promise<void>((resolve) => {
      resolveConnectGate = resolve
    })
    let connectStarted = false
    const originalConnect = host.connectClientInner.bind(host)
    host.connectClientInner = async (peerId: string) => {
      connectStarted = true
      await connectGate
      const lease = budget.acquire(peerId)
      host.sessions.set(
        peerId,
        createFakeSession({
          budgetLease: lease,
          pc: {
            connectionState: 'connected',
            close: vi.fn(),
            closeAsync: vi.fn(async () => undefined),
          },
        }),
      )
    }

    signaling.emit('peer-joined', 'client-race')
    await vi.waitFor(() => expect(connectStarted).toBe(true))

    // Real signaling close while connect is still gated.
    signaling.emit('peer-left', 'client-race')
    expect(host.sessions.has('client-race')).toBe(false)

    resolveConnectGate()
    await vi.waitFor(() => expect(host.sessions.has('client-race')).toBe(false))
    await vi.waitFor(() => expect(budget.snapshot().active).toBe(0))
    expect(originalConnect).toBeTypeOf('function')
  })

  it('peer-joined during gated close waits then acquires a new opaque lease', async () => {
    const signaling = new FakeSignaling()
    const budget = new VoiceSessionBudget(1)
    const host = createHost({}, budget, signaling)

    let resolveClose!: () => void
    const closeGate = new Promise<void>((resolve) => {
      resolveClose = resolve
    })
    const lease1 = budget.acquire('client-reconnect')
    host.sessions.set(
      'client-reconnect',
      createFakeSession({
        budgetLease: lease1,
        pc: {
          connectionState: 'connected',
          close: vi.fn(),
          closeAsync: vi.fn(() => closeGate),
        },
      }),
    )

    signaling.emit('peer-left', 'client-reconnect')
    await vi.waitFor(() => expect(host.sessions.has('client-reconnect')).toBe(false))
    expect(budget.snapshot().active).toBe(1)

    let connectEntered = false
    host.connectClientInner = async (peerId: string) => {
      connectEntered = true
      // Close must have released lease1 before a new acquire can succeed under max=1.
      expect(budget.snapshot().active).toBe(0)
      const lease2 = budget.acquire(peerId)
      expect(lease2).not.toBe(lease1)
      host.sessions.set(
        peerId,
        createFakeSession({
          budgetLease: lease2,
          pc: {
            connectionState: 'connected',
            close: vi.fn(),
            closeAsync: vi.fn(async () => undefined),
          },
        }),
      )
    }

    signaling.emit('peer-joined', 'client-reconnect')
    // Must wait for close — connect not entered while close gated.
    expect(connectEntered).toBe(false)
    resolveClose()
    await vi.waitFor(() => expect(connectEntered).toBe(true))
    await vi.waitFor(() => expect(host.sessions.has('client-reconnect')).toBe(true))
    expect(budget.snapshot().active).toBe(1)
  })

  it('partial createDataChannel failure closes pc and releases lease', async () => {
    const budget = new VoiceSessionBudget(1)
    const host = createHost({}, budget)
    const closeAsync = vi.fn(async () => undefined)
    host.connectClientBuildSession = async (_peerId, _lease, hooks) => {
      const pc = { closeAsync, close: vi.fn() }
      hooks.onPeerCreated(pc)
      throw new Error('createDataChannel boom')
    }

    await expect(host.connectClientInner('client-partial-dc')).rejects.toThrow(
      'createDataChannel boom',
    )
    expect(closeAsync).toHaveBeenCalledTimes(1)
    expect(budget.snapshot().active).toBe(0)
  })

  it('partial close timeout does not release lease', async () => {
    vi.useFakeTimers()
    try {
      const budget = new VoiceSessionBudget(1)
      const host = createHost({}, budget)
      const closeAsync = vi.fn(() => new Promise<void>(() => undefined))
      host.connectClientBuildSession = async (_peerId, _lease, hooks) => {
        const pc = { closeAsync, close: vi.fn() }
        hooks.onPeerCreated(pc)
        throw new Error('partial boom')
      }

      const failing = host.connectClientInner('client-partial-hang')
      const rejection = expect(failing).rejects.toThrow('partial boom')
      await vi.advanceTimersByTimeAsync(5_000)
      await rejection
      expect(budget.snapshot().active).toBe(1)
      expect(host.isRecycleRequired).toBe(true)
      expect(host.quarantinedCount).toBe(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('partial agent stop reject does not release lease', async () => {
    const budget = new VoiceSessionBudget(1)
    const host = createHost({}, budget)
    const stop = vi.fn(async () => {
      throw new Error('agent stop boom')
    })
    const closeAsync = vi.fn(async () => undefined)
    host.connectClientBuildSession = async (_peerId, _lease, hooks) => {
      const pc = { closeAsync, close: vi.fn() }
      hooks.onPeerCreated(pc)
      hooks.onAgentCreated({ stop } as never)
      throw new Error('addTrack boom')
    }

    await expect(host.connectClientInner('client-partial-agent-fail')).rejects.toThrow(
      'addTrack boom',
    )
    expect(stop).toHaveBeenCalledTimes(1)
    expect(closeAsync).toHaveBeenCalledTimes(1)
    expect(budget.snapshot().active).toBe(1)
    expect(host.isRecycleRequired).toBe(true)
  })

  it('partial addTrack failure stops agent, closes pc, releases lease concurrently', async () => {
    const budget = new VoiceSessionBudget(1)
    const host = createHost({}, budget)
    let resolveStop!: () => void
    const stopGate = new Promise<void>((resolve) => {
      resolveStop = resolve
    })
    let resolveClose!: () => void
    const closeGate = new Promise<void>((resolve) => {
      resolveClose = resolve
    })
    const stop = vi.fn(() => stopGate)
    const closeAsync = vi.fn(() => closeGate)
    host.connectClientBuildSession = async (_peerId, _lease, hooks) => {
      const pc = { closeAsync, close: vi.fn() }
      hooks.onPeerCreated(pc)
      const agent = { stop }
      hooks.onAgentCreated(agent)
      throw new Error('addTrack boom')
    }

    const failing = host.connectClientInner('client-partial-track')
    await vi.waitFor(() => expect(stop).toHaveBeenCalledTimes(1))
    await vi.waitFor(() => expect(closeAsync).toHaveBeenCalledTimes(1))
    // Concurrent under shared deadline — both in flight before either settles.
    expect(budget.snapshot().active).toBe(1)
    resolveStop()
    resolveClose()
    await expect(failing).rejects.toThrow('addTrack boom')
    expect(budget.snapshot().active).toBe(0)
  })

  it('deletes reconnectAttempts on teardown', async () => {
    const release = vi.fn()
    const host = createHost(
      {},
      {
        tryAcquire: () => 'lease-reconnect-map',
        release,
        snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
      },
    )
    host.reconnectAttempts.set('client-reconnect-map', 2)
    host.sessions.set(
      'client-reconnect-map',
      createFakeSession({
        budgetLease: 'lease-reconnect-map',
        pc: {
          connectionState: 'connected',
          close: vi.fn(),
          closeAsync: vi.fn(async () => undefined),
        },
      }),
    )
    await host.disconnectPeer('client-reconnect-map')
    expect(host.reconnectAttempts.has('client-reconnect-map')).toBe(false)
    expect(release).toHaveBeenCalledWith('lease-reconnect-map')
  })

  it('sync lifecycle hook throw does not prevent lease release', async () => {
    const release = vi.fn()
    const host = createHost(
      {
        onPeerDisconnected: () => {
          throw new Error('sync hook boom')
        },
      },
      {
        tryAcquire: () => 'lease-hook-sync',
        release,
        snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
      },
    )
    host.sessions.set(
      'client-hook-sync',
      createFakeSession({
        budgetLease: 'lease-hook-sync',
        pc: {
          connectionState: 'connected',
          close: vi.fn(),
          closeAsync: vi.fn(async () => undefined),
        },
      }),
    )
    await host.disconnectPeer('client-hook-sync')
    expect(release).toHaveBeenCalledWith('lease-hook-sync')
  })
})
