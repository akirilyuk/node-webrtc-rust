import { afterEach, describe, expect, it, vi } from 'vitest'

import { SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { SessionPod } from '../src/session-pod.js'
import { SessionPodCapacityFullError } from '../src/session-pod-errors.js'

describe('SessionPod concurrent prepare capacity', () => {
  let server: SignalingServer | undefined

  afterEach(async () => {
    vi.restoreAllMocks()
    if (server) {
      await server.close().catch(() => undefined)
      server = undefined
    }
  })

  it('rejects a third concurrent prepare when maxPreparedSessions is 2', async () => {
    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0)
    const port = server.port

    let releaseFirst!: () => void
    const firstGate = new Promise<void>((resolve) => {
      releaseFirst = resolve
    })

    const realConnect = SignalingClient.prototype.connect
    vi.spyOn(SignalingClient.prototype, 'connect').mockImplementation(async function (
      this: SignalingClient,
      ...args: Parameters<typeof realConnect>
    ) {
      if (vi.mocked(SignalingClient.prototype.connect).mock.calls.length === 1) {
        await firstGate
      }
      return realConnect.apply(this, args)
    })

    const pod = new SessionPod(server, {
      signalingUrl: `ws://127.0.0.1:${port}/ws`,
      iceServers: [],
      voiceConfig: {} as never,
      sessionMode: 'data-only',
      maxPreparedSessions: 2,
    })

    const first = pod.ensureSession('session-a')
    await Promise.resolve()
    const second = pod.ensureSession('session-b')
    await Promise.resolve()

    const third = pod.ensureSession('session-c')
    await expect(third).rejects.toBeInstanceOf(SessionPodCapacityFullError)

    releaseFirst()
    await first
    await second

    expect(pod.activeSessionCount).toBe(2)
    await pod.close().catch(() => undefined)
    server = undefined
  })
})
