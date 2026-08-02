import { describe, expect, it, vi } from 'vitest'

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
