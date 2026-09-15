import { afterEach, describe, expect, it, vi } from 'vitest'

import { SignalingServer } from '@node-webrtc-rust/signaling'

import {
  DEFAULT_NEVER_CONNECTED_REJOIN_GRACE_MS,
  DEFAULT_SESSION_REJOIN_GRACE_MS,
  SessionPod,
} from '../src/session-pod.js'
import type { VoiceSessionContext, VoiceSessionHandler } from '../src/voice-session-handler.js'

type SessionPodTestAccess = SessionPod & {
  wrapVoiceHandler: (
    sessionId: string,
    handler?: VoiceSessionHandler,
  ) => VoiceSessionHandler | undefined
  scheduleIdleTeardown: (sessionId: string, endReason?: string, graceMs?: number) => void
  maybeScheduleIdleTeardownAfterLastPeer: (sessionId: string, graceMs?: number) => void
  slots: Map<
    string,
    {
      sessionId: string
      host: { activeClientCount: number }
      signaling: { emit: (event: string, peerId: string) => void }
    }
  >
}

function seedIdleSlot(pod: SessionPodTestAccess, sessionId: string): void {
  pod.slots.set(sessionId, {
    sessionId,
    host: { activeClientCount: 0 },
  })
}

function createPod(
  overrides: {
    rejoinGraceMs?: number
    neverConnectedRejoinGraceMs?: number
  } = {},
): SessionPodTestAccess {
  return new SessionPod({} as never, {
    signalingUrl: 'ws://127.0.0.1/ws',
    iceServers: [],
    voiceConfig: {} as never,
    teardownIdleSessions: true,
    ...overrides,
  }) as SessionPodTestAccess
}

describe('SessionPod rejoin grace', () => {
  it('uses never-connected grace for onPeerSignalingLost', () => {
    vi.useFakeTimers()
    try {
      const pod = createPod()
      seedIdleSlot(pod, 'session-never')
      const scheduleIdleTeardown = vi.spyOn(pod, 'scheduleIdleTeardown')
      const wrapped = pod.wrapVoiceHandler('session-never')
      const ctx = { peerId: 'client-early' } as VoiceSessionContext

      wrapped?.onPeerSignalingLost?.(ctx)
      vi.runAllTimers()

      expect(scheduleIdleTeardown).toHaveBeenCalledWith(
        'session-never',
        undefined,
        DEFAULT_NEVER_CONNECTED_REJOIN_GRACE_MS,
      )
    } finally {
      vi.useRealTimers()
    }
  })

  it('uses default rejoin grace for onPeerDisconnected', () => {
    vi.useFakeTimers()
    try {
      const pod = createPod()
      seedIdleSlot(pod, 'session-post')
      const scheduleIdleTeardown = vi.spyOn(pod, 'scheduleIdleTeardown')
      const wrapped = pod.wrapVoiceHandler('session-post')
      const ctx = { peerId: 'client-done' } as VoiceSessionContext

      wrapped?.onPeerDisconnected?.(ctx)
      vi.runAllTimers()

      expect(scheduleIdleTeardown).toHaveBeenCalledWith(
        'session-post',
        undefined,
        DEFAULT_SESSION_REJOIN_GRACE_MS,
      )
    } finally {
      vi.useRealTimers()
    }
  })

  it('respects custom neverConnectedRejoinGraceMs', () => {
    vi.useFakeTimers()
    try {
      const pod = createPod({ neverConnectedRejoinGraceMs: 90_000 })
      seedIdleSlot(pod, 'session-custom')
      const scheduleIdleTeardown = vi.spyOn(pod, 'scheduleIdleTeardown')
      const wrapped = pod.wrapVoiceHandler('session-custom')
      const ctx = { peerId: 'client-custom' } as VoiceSessionContext

      wrapped?.onPeerSignalingLost?.(ctx)
      vi.runAllTimers()

      expect(scheduleIdleTeardown).toHaveBeenCalledWith('session-custom', undefined, 90_000)
    } finally {
      vi.useRealTimers()
    }
  })
})

describe('SessionPod prepare never-connected teardown', () => {
  let server: SignalingServer | undefined

  afterEach(async () => {
    vi.useRealTimers()
    vi.restoreAllMocks()
    if (server) {
      await server.close().catch(() => undefined)
      server = undefined
    }
  })

  it('arms never-connected teardown after prepareSessionSlot', async () => {
    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0)
    const port = server.port

    const pod = new SessionPod(server, {
      signalingUrl: `ws://127.0.0.1:${port}/ws`,
      iceServers: [],
      voiceConfig: {} as never,
      sessionMode: 'data-only',
      teardownIdleSessions: true,
    }) as SessionPodTestAccess
    const scheduleIdleTeardown = vi.spyOn(pod, 'scheduleIdleTeardown')

    await pod.ensureSession('session-prepare')

    expect(scheduleIdleTeardown).toHaveBeenCalledWith(
      'session-prepare',
      'never_connected',
      DEFAULT_NEVER_CONNECTED_REJOIN_GRACE_MS,
    )
    await pod.close().catch(() => undefined)
    server = undefined
  })

  it('tears down prepared slot after never-connected grace with no joins', async () => {
    vi.useFakeTimers()
    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0)
    const port = server.port

    const pod = new SessionPod(server, {
      signalingUrl: `ws://127.0.0.1:${port}/ws`,
      iceServers: [],
      voiceConfig: {} as never,
      sessionMode: 'data-only',
      teardownIdleSessions: true,
    })

    await pod.ensureSession('session-prepare')
    expect(pod.activeSessionCount).toBe(1)

    await vi.advanceTimersByTimeAsync(DEFAULT_NEVER_CONNECTED_REJOIN_GRACE_MS)
    await vi.waitFor(() => expect(pod.activeSessionCount).toBe(0))

    await pod.close().catch(() => undefined)
    server = undefined
  })

  it('cancels prepare never-connected teardown on onPeerConnected', async () => {
    vi.useFakeTimers()
    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0)
    const port = server.port

    const handler: VoiceSessionHandler = {
      onPeerConnected: vi.fn(),
    }
    const pod = new SessionPod(server, {
      signalingUrl: `ws://127.0.0.1:${port}/ws`,
      iceServers: [],
      voiceConfig: {} as never,
      sessionMode: 'data-only',
      teardownIdleSessions: true,
      voiceHandler: handler,
    }) as SessionPodTestAccess

    await pod.ensureSession('session-prepare')
    const wrapped = pod.wrapVoiceHandler('session-prepare', handler)
    await wrapped?.onPeerConnected?.({ peerId: 'client-a' } as VoiceSessionContext)

    await vi.advanceTimersByTimeAsync(DEFAULT_NEVER_CONNECTED_REJOIN_GRACE_MS)
    expect(pod.activeSessionCount).toBe(1)

    await pod.close().catch(() => undefined)
    server = undefined
  })

  it('cancels prepare never-connected teardown on peer-joined client', async () => {
    vi.useFakeTimers()
    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0)
    const port = server.port

    const pod = new SessionPod(server, {
      signalingUrl: `ws://127.0.0.1:${port}/ws`,
      iceServers: [],
      voiceConfig: {} as never,
      sessionMode: 'data-only',
      teardownIdleSessions: true,
    }) as SessionPodTestAccess

    await pod.ensureSession('session-prepare')
    pod.slots.get('session-prepare')?.signaling.emit('peer-joined', 'client-a')

    await vi.advanceTimersByTimeAsync(DEFAULT_NEVER_CONNECTED_REJOIN_GRACE_MS)
    expect(pod.activeSessionCount).toBe(1)

    await pod.close().catch(() => undefined)
    server = undefined
  })
})
