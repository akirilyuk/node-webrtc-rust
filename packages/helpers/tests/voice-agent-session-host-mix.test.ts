import { describe, expect, it, vi } from 'vitest'

import type { ClientMixGraph } from '../src/client-audio-mixer.js'
import { PCM_FULL_FRAME_BYTES } from '../src/pcm.js'
import {
  MIX_REQUIRES_VOICE_PLUS_DATA,
  VoiceAgentSessionHost,
} from '../src/voice-agent-session-host.js'

function createMockMixGraph(): ClientMixGraph {
  const silence = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  return {
    addInput: vi.fn(),
    removeInput: vi.fn(),
    pushFrame: vi.fn(),
    renderOutput: vi.fn(() => Buffer.from(silence)),
    panTtsFrame: vi.fn((pcm: Buffer) => Buffer.from(pcm)),
    setPose: vi.fn(),
    setPositionalEnabled: vi.fn(),
    setDefaultMixPlacement: vi.fn(),
    setTtsMixPlacement: vi.fn(),
    setGroupMembers: vi.fn(),
    moveToGroup: vi.fn(),
    removeFromGroup: vi.fn(),
  }
}

type FakeAgent = {
  attach: ReturnType<typeof vi.fn>
  start: ReturnType<typeof vi.fn>
  stop: ReturnType<typeof vi.fn>
  sendTextToTTS: ReturnType<typeof vi.fn>
  setSttEnabled: ReturnType<typeof vi.fn>
  on: ReturnType<typeof vi.fn>
  speechEvents: () => AsyncGenerator<never, void, unknown>
}

type HostTestAccess = VoiceAgentSessionHost & {
  sessions: Map<
    string,
    {
      agent?: FakeAgent
      agentStarted: boolean
    }
  >
}

function createHost(
  sessionMode: 'voice' | 'voice+data' | 'data-only',
  clientMixGraph?: ClientMixGraph,
): HostTestAccess {
  const host = new VoiceAgentSessionHost(createStubSignaling() as never, [], {
    voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } } as never,
    sessionMode,
    clientMixGraph,
    sessionBudget: {
      tryAcquire: () => 'lease-test',
      release: () => undefined,
      snapshot: () => ({ active: 0, max: 0, available: 0, rejectedTotal: 0 }),
    },
  })
  return host as unknown as HostTestAccess
}

function createStubSignaling() {
  return { room: 'test-room', on: vi.fn() }
}

function fakeAgent(): FakeAgent {
  return {
    attach: vi.fn(async () => undefined),
    start: vi.fn(async () => undefined),
    stop: vi.fn(async () => undefined),
    sendTextToTTS: vi.fn(async () => undefined),
    setSttEnabled: vi.fn(async () => undefined),
    on: vi.fn(() => () => undefined),
    speechEvents: async function* () {
      /* empty */
    },
  }
}

describe('VoiceAgentSessionHost mix APIs', () => {
  it('throws on voice-only session mode', () => {
    const host = createHost('voice')
    expect(() => host.createMixGroup({ id: 'g1', clientIds: ['a'] })).toThrow(
      MIX_REQUIRES_VOICE_PLUS_DATA,
    )
  })

  it('throws on data-only session mode', () => {
    const host = createHost('data-only')
    expect(() => host.setPositionalMixing(true)).toThrow(MIX_REQUIRES_VOICE_PLUS_DATA)
  })

  it('creates isolated groups and supports mid-call exclusive move', () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)

    host.createMixGroup({ id: 'team1', clientIds: ['A', 'B', 'C'] })
    host.createMixGroup({ id: 'team2', clientIds: ['D', 'F'] })

    expect(graph.setGroupMembers).toHaveBeenCalledWith('team1', ['A', 'B', 'C'])
    expect(graph.setGroupMembers).toHaveBeenCalledWith('team2', ['D', 'F'])

    host.addClientToMix('team2', 'A')
    expect(graph.moveToGroup).toHaveBeenCalledWith('A', 'team2')
  })

  it('removeClientFromMix leaves client ungrouped', () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    host.removeClientFromMix('team1', 'A')
    expect(graph.removeFromGroup).toHaveBeenCalledWith('A')
  })

  it('forwards pose and placement controls to the graph', () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    const pose = {
      position: { x: 1, y: 0, z: 0 },
      orientation: { x: 0, y: 0, z: 0, w: 1 },
    }

    host.setClientPose('A', pose)
    host.setPositionalMixing(true)
    host.setDefaultMixPlacement('left')
    host.setTtsMixPlacement('right')

    expect(graph.setPose).toHaveBeenCalledWith('A', pose)
    expect(graph.setPositionalEnabled).toHaveBeenCalledWith(true)
    expect(graph.setDefaultMixPlacement).toHaveBeenCalledWith('left')
    expect(graph.setTtsMixPlacement).toHaveBeenCalledWith('right')
  })

  it('setSttEnabled toggles one client or all agents', async () => {
    const host = createHost('voice+data', createMockMixGraph())
    const agentA = fakeAgent()
    const agentB = fakeAgent()
    host.sessions.set('client-a', { agent: agentA, agentStarted: true })
    host.sessions.set('client-b', { agent: agentB, agentStarted: true })

    await host.setSttEnabled({ enabled: false, clientId: 'client-a' })
    expect(agentA.setSttEnabled).toHaveBeenCalledWith(false)
    expect(agentB.setSttEnabled).not.toHaveBeenCalled()

    await host.setSttEnabled({ enabled: true })
    expect(agentA.setSttEnabled).toHaveBeenCalledWith(true)
    expect(agentB.setSttEnabled).toHaveBeenCalledWith(true)
  })
})
