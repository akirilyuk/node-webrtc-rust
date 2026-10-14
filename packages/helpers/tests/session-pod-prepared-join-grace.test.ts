import { afterEach, describe, expect, it, vi } from 'vitest'

import { SignalingServer } from '@node-webrtc-rust/signaling'

import { SessionPod, type SessionPodOptions } from '../src/session-pod.js'

function ids(p: SessionPod): string[] {
  return p.listSessions().map((s) => s.sessionId)
}

interface PodInternals {
  cancelTeardownTimer(sessionId: string): void
  wrapVoiceHandler(
    sessionId: string,
    handler?: unknown,
  ): { onPeerSignalingLost?: (ctx: unknown) => unknown } | undefined
}

describe('SessionPod prepared-session join grace', () => {
  let server: SignalingServer | undefined
  let pod: SessionPod | undefined

  afterEach(async () => {
    vi.useRealTimers()
    await pod?.close().catch(() => undefined)
    pod = undefined
    if (server) {
      await server.close().catch(() => undefined)
      server = undefined
    }
  })

  async function startPod(
    extra: Partial<SessionPodOptions>,
    ends: Array<string | undefined>,
  ): Promise<SessionPod> {
    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0, '127.0.0.1')
    pod = new SessionPod(server, {
      signalingUrl: `ws://127.0.0.1:${server.port}/ws`,
      iceServers: [],
      voiceConfig: {} as never,
      sessionMode: 'data-only',
      onSessionChange: (event) => {
        if (event.action === 'destroyed') ends.push(event.endReason)
      },
      ...extra,
    })
    return pod
  }

  it('prepared session without a client is torn down after neverConnectedRejoinGraceMs by default', async () => {
    const ends: Array<string | undefined> = []
    const p = await startPod({ neverConnectedRejoinGraceMs: 60_000 }, ends)
    vi.useFakeTimers()
    await p.ensureSession('s1')
    expect(ids(p)).toContain('s1')

    await vi.advanceTimersByTimeAsync(59_999)
    expect(ids(p)).toContain('s1')
    await vi.advanceTimersByTimeAsync(1)
    await vi.advanceTimersByTimeAsync(0)
    expect(ids(p)).not.toContain('s1')
    expect(ends).toEqual(['never_connected'])
  })

  it('preparedSessionJoinGraceMs overrides the creation grace', async () => {
    const ends: Array<string | undefined> = []
    const p = await startPod(
      { neverConnectedRejoinGraceMs: 60_000, preparedSessionJoinGraceMs: 180_000 },
      ends,
    )
    vi.useFakeTimers()
    await p.ensureSession('s1')

    await vi.advanceTimersByTimeAsync(60_000)
    expect(ids(p)).toContain('s1')
    await vi.advanceTimersByTimeAsync(119_999)
    expect(ids(p)).toContain('s1')
    await vi.advanceTimersByTimeAsync(1)
    await vi.advanceTimersByTimeAsync(0)
    expect(ids(p)).not.toContain('s1')
    expect(ends).toEqual(['never_connected'])
  })

  it('preparedSessionJoinGraceMs does not change the signaling-lost grace', async () => {
    const ends: Array<string | undefined> = []
    const p = await startPod(
      { neverConnectedRejoinGraceMs: 60_000, preparedSessionJoinGraceMs: 180_000 },
      ends,
    )
    vi.useFakeTimers()
    await p.ensureSession('s1')

    // A client joined (creation timer cancelled), then its signaling dropped before transport.
    const internals = p as unknown as PodInternals
    internals.cancelTeardownTimer('s1')
    internals.wrapVoiceHandler('s1')?.onPeerSignalingLost?.({})
    await vi.advanceTimersByTimeAsync(0)

    await vi.advanceTimersByTimeAsync(59_000)
    expect(ids(p)).toContain('s1')
    await vi.advanceTimersByTimeAsync(1_000)
    await vi.advanceTimersByTimeAsync(0)
    expect(ids(p)).not.toContain('s1')
  })
})
