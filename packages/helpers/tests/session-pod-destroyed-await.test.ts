import { afterEach, describe, expect, it, vi } from 'vitest'

import { SignalingServer } from '@node-webrtc-rust/signaling'

import { SessionPod } from '../src/session-pod.js'

describe('SessionPod awaits destroyed onSessionChange', () => {
  let server: SignalingServer | undefined

  afterEach(async () => {
    if (server) {
      await server.close().catch(() => undefined)
      server = undefined
    }
  })

  it('keeps slot while awaiting destroyed; deletes only after success', async () => {
    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0)
    const port = server.port

    const order: string[] = []
    let releaseDestroyed!: () => void
    const destroyedGate = new Promise<void>((resolve) => {
      releaseDestroyed = resolve
    })
    let midHookActive = -1
    let eventActiveSessions = -1

    const pod = new SessionPod(server, {
      signalingUrl: `ws://127.0.0.1:${port}/ws`,
      iceServers: [],
      voiceConfig: {} as never,
      sessionMode: 'data-only',
      teardownIdleSessions: false,
      onSessionChange: async (event) => {
        if (event.action !== 'destroyed') return
        order.push('destroyed-start')
        midHookActive = pod.activeSessionCount
        eventActiveSessions = event.activeSessions
        await destroyedGate
        order.push('destroyed-done')
      },
    })

    await pod.ensureSession('session-await')
    expect(pod.activeSessionCount).toBe(1)

    const teardown = pod.teardownSession('session-await', 'test')
    await Promise.resolve()
    await Promise.resolve()
    expect(order).toEqual(['destroyed-start'])
    expect(midHookActive).toBe(1)
    expect(eventActiveSessions).toBe(0)
    expect(pod.activeSessionCount).toBe(1)

    releaseDestroyed()
    await teardown
    expect(order).toEqual(['destroyed-start', 'destroyed-done'])
    expect(pod.activeSessionCount).toBe(0)

    await pod.close().catch(() => undefined)
    server = undefined
  })

  it('retains slot when destroyed callback rejects', async () => {
    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0)
    const port = server.port

    const pod = new SessionPod(server, {
      signalingUrl: `ws://127.0.0.1:${port}/ws`,
      iceServers: [],
      voiceConfig: {} as never,
      sessionMode: 'data-only',
      teardownIdleSessions: false,
      onSessionChange: async (event) => {
        if (event.action === 'destroyed') {
          throw new Error('session_end failed')
        }
      },
    })

    await pod.ensureSession('session-fail')
    await expect(pod.teardownSession('session-fail')).rejects.toThrow(
      /session_end failed/,
    )
    expect(pod.activeSessionCount).toBe(1)

    await pod.close().catch(() => undefined)
    server = undefined
  })

  it('still supports synchronous destroyed callbacks', async () => {
    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0)
    const port = server.port
    let destroyed = false

    const pod = new SessionPod(server, {
      signalingUrl: `ws://127.0.0.1:${port}/ws`,
      iceServers: [],
      voiceConfig: {} as never,
      sessionMode: 'data-only',
      teardownIdleSessions: false,
      onSessionChange: (event) => {
        if (event.action === 'destroyed') destroyed = true
      },
    })

    await pod.ensureSession('session-sync')
    await pod.teardownSession('session-sync')
    expect(destroyed).toBe(true)
    expect(pod.activeSessionCount).toBe(0)
    await pod.close().catch(() => undefined)
    server = undefined
  })

  it('does not double-count quarantine when destroyed hook rejects', async () => {
    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0)
    const port = server.port

    type PodAccess = SessionPod & {
      slots: Map<string, { host: { quarantinedCount: number; isRecycleRequired: boolean } }>
      retiredHosts: Map<string, { quarantinedCount: number; isRecycleRequired: boolean }>
    }

    const pod = new SessionPod(server, {
      signalingUrl: `ws://127.0.0.1:${port}/ws`,
      iceServers: [],
      voiceConfig: {} as never,
      sessionMode: 'data-only',
      teardownIdleSessions: false,
      onSessionChange: async (event) => {
        if (event.action === 'destroyed') {
          throw new Error('session_end failed')
        }
      },
    }) as unknown as PodAccess

    await pod.ensureSession('session-q')
    const slot = pod.slots.get('session-q')
    expect(slot).toBeDefined()
    vi.spyOn(slot!.host as never, 'close').mockImplementation(async () => {
      ;(slot!.host as unknown as { recycleRequired: boolean }).recycleRequired =
        true
      ;(
        slot!.host as unknown as { quarantinedLeases: Set<string> }
      ).quarantinedLeases = new Set(['zombie'])
    })

    await expect(pod.teardownSession('session-q')).rejects.toThrow(
      /session_end failed/,
    )
    expect(pod.activeSessionCount).toBe(1)
    expect(pod.retiredHosts.has('session-q')).toBe(false)
    // Live slot only — not also in retiredHosts.
    expect(pod.quarantinedPeerCount).toBe(1)
    expect(pod.isRecycleRequired).toBe(true)

    await pod.close().catch(() => undefined)
    server = undefined
  })

  it('deferred publish after destroyed observes slot deleted', async () => {
    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0)
    const port = server.port
    let publishedActive = -1

    const pod = new SessionPod(server, {
      signalingUrl: `ws://127.0.0.1:${port}/ws`,
      iceServers: [],
      voiceConfig: {} as never,
      sessionMode: 'data-only',
      teardownIdleSessions: false,
      onSessionChange: async (event) => {
        if (event.action !== 'destroyed') return
        // Mirror runner main: defer publish until after slot deletion.
        setTimeout(() => {
          publishedActive = pod.activeSessionCount
        }, 0)
      },
    })

    await pod.ensureSession('session-pub')
    await pod.teardownSession('session-pub')
    expect(pod.activeSessionCount).toBe(0)
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(publishedActive).toBe(0)

    await pod.close().catch(() => undefined)
    server = undefined
  })
})
