import { describe, expect, it, vi } from 'vitest'

import { SessionPod } from '../src/session-pod.js'
import type { VoiceSessionContext, VoiceSessionHandler } from '../src/voice-session-handler.js'

type SessionPodTestAccess = SessionPod & {
  wrapVoiceHandler: (
    sessionId: string,
    handler?: VoiceSessionHandler,
  ) => VoiceSessionHandler | undefined
  cancelTeardownTimer: (sessionId: string) => void
}

describe('SessionPod voice handler lifecycle', () => {
  it('forwards both readiness phases and cancels idle teardown for each', async () => {
    const calls: string[] = []
    const handler: VoiceSessionHandler = {
      onPeerTransportReady: () => {
        calls.push('transport')
      },
      onPeerConnected: () => {
        calls.push('connected')
      },
    }
    const pod = new SessionPod({} as never, {
      signalingUrl: 'ws://127.0.0.1/ws',
      iceServers: [],
      voiceConfig: {} as never,
      teardownIdleSessions: true,
    }) as SessionPodTestAccess
    const cancelTeardownTimer = vi
      .spyOn(pod, 'cancelTeardownTimer')
      .mockImplementation(() => undefined)
    const wrapped = pod.wrapVoiceHandler('session-a', handler)
    const ctx = { peerId: 'client-a' } as VoiceSessionContext

    await wrapped?.onPeerTransportReady?.(ctx)
    await wrapped?.onPeerConnected?.(ctx)

    expect(calls).toEqual(['transport', 'connected'])
    expect(cancelTeardownTimer).toHaveBeenNthCalledWith(1, 'session-a')
    expect(cancelTeardownTimer).toHaveBeenNthCalledWith(2, 'session-a')
  })
})
