import { afterEach, describe, expect, it, vi } from 'vitest'

import { SignalingServer } from '@node-webrtc-rust/signaling'

import { SessionPod } from '../src/session-pod.js'
import { SessionPodRecycleRequiredError } from '../src/session-pod-errors.js'
import type { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'

type PodAccess = SessionPod & {
  slots: Map<
    string,
    {
      sessionId: string
      host: VoiceAgentSessionHost
      signaling: { disconnect: () => void }
    }
  >
  retiredHosts: Map<string, VoiceAgentSessionHost>
}

describe('SessionPod sticky quarantine + single-flight teardown', () => {
  let server: SignalingServer | undefined

  afterEach(async () => {
    vi.restoreAllMocks()
    if (server) {
      await server.close().catch(() => undefined)
      server = undefined
    }
  })

  async function makePod(
    onSessionChange?: (event: {
      sessionId: string
      action: 'created' | 'destroyed'
      activeSessions: number
    }) => void,
  ): Promise<PodAccess> {
    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0)
    const port = server.port
    const pod = new SessionPod(server, {
      signalingUrl: `ws://127.0.0.1:${port}/ws`,
      iceServers: [],
      voiceConfig: {} as never,
      sessionMode: 'data-only',
      teardownIdleSessions: false,
      onSessionChange,
    })
    return pod as unknown as PodAccess
  }

  it('keeps recycle required after slot teardown when host is quarantined', async () => {
    const pod = await makePod()
    await pod.ensureSession('session-quarantine')
    const slot = pod.slots.get('session-quarantine')
    expect(slot).toBeDefined()
    const host = slot!.host

    vi.spyOn(host, 'close').mockImplementation(async () => {
      ;(host as unknown as { recycleRequired: boolean }).recycleRequired = true
      ;(host as unknown as { quarantinedLeases: Set<string> }).quarantinedLeases.add('zombie-lease')
    })
    // Keep real signaling.disconnect so the WS does not leak open handles.

    await pod.teardownSession('session-quarantine')
    expect(pod.activeSessionCount).toBe(0)
    expect(pod.retiredHosts.has('session-quarantine')).toBe(true)
    expect(pod.isRecycleRequired).toBe(true)
    expect(pod.quarantinedPeerCount).toBe(1)

    await expect(pod.ensureSession('session-next')).rejects.toBeInstanceOf(
      SessionPodRecycleRequiredError,
    )

    const closeOutcome = await pod.close()
    expect(closeOutcome.recycleRequired).toBe(true)
    expect(closeOutcome.quarantined).toBeGreaterThanOrEqual(1)
    server = undefined
  })

  it('overlapping teardown calls host.close and emits destroy exactly once', async () => {
    const destroyEvents: string[] = []
    const pod = await makePod((event) => {
      if (event.action === 'destroyed') {
        destroyEvents.push(event.sessionId)
      }
    })
    await pod.ensureSession('session-once')
    const slot = pod.slots.get('session-once')
    expect(slot).toBeDefined()

    let resolveClose!: () => void
    const closeGate = new Promise<void>((resolve) => {
      resolveClose = resolve
    })
    const close = vi.fn(() => closeGate)
    vi.spyOn(slot!.host, 'close').mockImplementation(close)

    const a = pod.teardownSession('session-once', 'idle')
    const b = pod.teardownSession('session-once', 'forced')
    const c = pod.teardownSession('session-once', 'drain')
    expect(close).toHaveBeenCalledTimes(1)

    resolveClose()
    await Promise.all([a, b, c])
    expect(close).toHaveBeenCalledTimes(1)
    expect(destroyEvents).toEqual(['session-once'])
    expect(pod.activeSessionCount).toBe(0)

    await pod.close()
    server = undefined
  })
})
