import { afterEach, describe, expect, it, vi } from 'vitest'

import { SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { SessionPod } from '../src/session-pod.js'

type SessionPodTestAccess = SessionPod & {
  slots: Map<string, { signaling: SignalingClient }>
}

type SignalingClientTestAccess = SignalingClient & {
  ws: { close: () => void } | null
}

describe('SessionPod agent signaling reconnect', () => {
  let server: SignalingServer | undefined

  afterEach(async () => {
    vi.restoreAllMocks()
    if (server) {
      await server.close().catch(() => undefined)
      server = undefined
    }
  })

  it('reconnects the agent signaling client after websocket drop', async () => {
    const connectSpy = vi.spyOn(SignalingClient.prototype, 'connect')

    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0)
    const port = server.port
    const signalingUrl = `ws://127.0.0.1:${port}/ws`

    const pod = new SessionPod(server, {
      signalingUrl,
      iceServers: [],
      voiceConfig: {} as never,
      sessionMode: 'data-only',
      maxPreparedSessions: 4,
    })

    await pod.ensureSession('session-a')
    expect(connectSpy).toHaveBeenCalledTimes(1)

    const agentSignaling = (pod as SessionPodTestAccess).slots.get('session-a')?.signaling as
      | SignalingClientTestAccess
      | undefined
    expect(agentSignaling?.ws).toBeTruthy()
    agentSignaling!.ws!.close()

    await vi.waitFor(
      () => {
        expect(connectSpy).toHaveBeenCalledTimes(2)
      },
      { timeout: 5_000, interval: 50 },
    )

    await pod.close().catch(() => undefined)
    server = undefined
  })
})
