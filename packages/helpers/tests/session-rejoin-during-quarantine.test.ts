import { describe, expect, it, vi } from 'vitest'

import { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'

type HostAccess = VoiceAgentSessionHost & {
  quarantinedLeases: Set<string>
  quarantineWaits: Map<string, { peerId: string; pc: string; agent: string }>
  recycleRequired: boolean
  connectClientBuildSession: ReturnType<typeof vi.fn>
  connectClientInner: (peerId: string) => Promise<void>
}

function createHost(logs: string[], lease: string | null = 'lease-new'): HostAccess {
  const signaling = {
    room: 'test-room',
    on: vi.fn(),
    sendOffer: vi.fn(),
    sendIceCandidate: vi.fn(),
  }
  const host = new VoiceAgentSessionHost(signaling as never, [], {
    voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } } as never,
    sessionMode: 'data-only',
    log: (m) => logs.push(m),
    sessionBudget: {
      tryAcquire: () => lease,
      release: () => undefined,
      snapshot: () => ({ active: 1, max: 1, available: 0, rejectedTotal: 0 }),
    },
  }) as unknown as HostAccess
  host.quarantinedLeases.add('lease-stale')
  host.quarantineWaits.set('lease-stale', { peerId: 'client-1', pc: 'pending', agent: 'ok' })
  host.recycleRequired = true
  host.connectClientBuildSession = vi.fn().mockResolvedValue(undefined)
  return host
}

describe('rejoin while host recycle is required', () => {
  it('rejoin of the quarantined peer is admitted while recycle is required', async () => {
    const logs: string[] = []
    const host = createHost(logs)
    await host.connectClientInner('client-1')
    expect(host.connectClientBuildSession).toHaveBeenCalledTimes(1)
    expect(logs.some((l) => l.includes('connect allowed during recycle'))).toBe(true)
    expect(host.recycleRequired).toBe(true)
  })

  it('a different peer is still refused while recycle is required', async () => {
    const logs: string[] = []
    const host = createHost(logs)
    await host.connectClientInner('client-2')
    expect(host.connectClientBuildSession).not.toHaveBeenCalled()
    expect(logs.some((l) => l.includes('connect skipped — host recycle required'))).toBe(true)
  })

  it('budget still applies to the admitted rejoin', async () => {
    const logs: string[] = []
    const host = createHost(logs, null)
    await host.connectClientInner('client-1')
    expect(host.connectClientBuildSession).not.toHaveBeenCalled()
    expect(logs.some((l) => l.includes('rejected — session budget full'))).toBe(true)
  })
})
