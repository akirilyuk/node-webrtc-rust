import { describe, expect, it, vi } from 'vitest'

import type { ClientAudioMixer, ClientMixGraph } from '../src/client-audio-mixer.js'
import { PCM_FULL_FRAME_BYTES } from '../src/pcm.js'
import {
  MIX_REQUIRES_VOICE_PLUS_DATA,
  TTS_POSE_REQUIRES_VOICE,
  VoiceAgentSessionHost,
} from '../src/voice-agent-session-host.js'

function createMockMixGraph(): ClientMixGraph {
  const silence = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  const globalMute = new Map<string, boolean>()
  const listenerMute = new Map<string, boolean>()
  return {
    addInput: vi.fn(),
    removeInput: vi.fn(),
    pushFrame: vi.fn(),
    renderOutput: vi.fn(() => Buffer.from(silence)),
    panTtsFrame: vi.fn((pcm: Buffer, _listenerId: string) => Buffer.from(pcm)),
    setPose: vi.fn(),
    setPositionalEnabled: vi.fn(),
    setDefaultMixPlacement: vi.fn(),
    setTtsMixPlacement: vi.fn(),
    setTtsPose: vi.fn(async () => undefined),
    clearTtsPose: vi.fn(async () => undefined),
    setGroupMembers: vi.fn(),
    moveToGroup: vi.fn(),
    removeFromGroup: vi.fn(),
    setGlobalMute: vi.fn((target: string, muted: boolean) => {
      globalMute.set(target, muted)
    }),
    isGloballyMuted: vi.fn((target: string) => globalMute.get(target) ?? false),
    setListenerMute: vi.fn((listener: string, target: string, muted: boolean) => {
      listenerMute.set(`${listener}:${target}`, muted)
    }),
    isListenerMuted: vi.fn(
      (listener: string, target: string) => listenerMute.get(`${listener}:${target}`) ?? false,
    ),
    pose: vi.fn(() => null),
    ttsPose: vi.fn(() => null),
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
  clientMixer?: ClientAudioMixer
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

function createStubSignaling(room = 'test-room') {
  return { room, on: vi.fn() }
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
  it('throws mix groups on voice-only session mode', () => {
    const host = createHost('voice', createMockMixGraph())
    expect(() => host.createMixGroup({ id: 'g1', clientIds: ['a'] })).toThrow(
      MIX_REQUIRES_VOICE_PLUS_DATA,
    )
  })

  it('lazy ensureClientMixer reuses injected clientMixGraph', () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    host.clientMixer = undefined

    host.createMixGroup({ id: 'g1', clientIds: ['a'] })

    expect(graph.setGroupMembers).toHaveBeenCalledWith('g1', ['a'])
    expect(host.getClientMixer()?.getMixGraph()).toBe(graph)
  })

  it('lazy ensureClientMixer wrapInboundTrack tees into injected graph', async () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    host.clientMixer = undefined
    host.createMixGroup({ id: 'g1', clientIds: ['client-a'] })

    const mixer = host.getClientMixer()
    expect(mixer?.getMixGraph()).toBe(graph)

    const pcm = Buffer.alloc(PCM_FULL_FRAME_BYTES, 1)
    const track = { readSample: vi.fn(async () => pcm) }
    mixer!.registerPeer('client-a')
    mixer!.wrapInboundTrack('client-a', track as never)
    await track.readSample()

    expect(graph.pushFrame).toHaveBeenCalledWith(
      'client-a',
      expect.objectContaining({ length: PCM_FULL_FRAME_BYTES }),
    )
  })

  it('forwards TTS pose controls on voice-only when clientMixGraph is injected', async () => {
    const graph = createMockMixGraph()
    const host = createHost('voice', graph)
    const pose = {
      position: { x: 1, y: 0, z: 0 },
      orientation: { x: 0, y: 0, z: 0, w: 1 },
    }

    host.setClientPose('A', pose)
    host.setPositionalMixing(true)
    host.setTtsMixPlacement('right')
    await host.setTtsPose('A', pose)
    await host.clearTtsPose('A')

    expect(graph.setPose).toHaveBeenCalledWith('A', pose)
    expect(graph.setPositionalEnabled).toHaveBeenCalledWith(true)
    expect(graph.setTtsMixPlacement).toHaveBeenCalledWith('right')
    expect(graph.setTtsPose).toHaveBeenCalledWith('A', pose)
    expect(graph.clearTtsPose).toHaveBeenCalledWith('A')
  })

  it('throws on data-only session mode for mix groups', () => {
    const host = createHost('data-only')
    expect(() => host.createMixGroup({ id: 'g1', clientIds: ['a'] })).toThrow(
      MIX_REQUIRES_VOICE_PLUS_DATA,
    )
  })

  it('throws on data-only session mode for TTS pose', () => {
    const host = createHost('data-only')
    expect(() => host.setPositionalMixing(true)).toThrow(TTS_POSE_REQUIRES_VOICE)
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

  it('global mute disables STT for the target client only', async () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    const agentA = fakeAgent()
    const agentB = fakeAgent()
    host.sessions.set('client-a', { agent: agentA, agentStarted: true })
    host.sessions.set('client-b', { agent: agentB, agentStarted: true })

    await host.setGlobalMute('client-a', true)
    expect(graph.setGlobalMute).toHaveBeenCalledWith('client-a', true)
    expect(agentA.setSttEnabled).toHaveBeenCalledWith(false)
    expect(agentB.setSttEnabled).not.toHaveBeenCalled()
  })

  it('global mute with sttEnabled true keeps STT on', async () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    const agentA = fakeAgent()
    host.sessions.set('client-a', { agent: agentA, agentStarted: true })

    await host.setGlobalMute('client-a', true, { sttEnabled: true })
    expect(agentA.setSttEnabled).toHaveBeenCalledWith(true)
    expect(host.getClientMixStatus('client-a').sttEnabled).toBe(true)
  })

  it('setSttEnabled while globally muted records explicit override', async () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    const agentA = fakeAgent()
    host.sessions.set('client-a', { agent: agentA, agentStarted: true })

    await host.setGlobalMute('client-a', true)
    agentA.setSttEnabled.mockClear()
    await host.setSttEnabled({ enabled: true, clientId: 'client-a' })
    expect(agentA.setSttEnabled).toHaveBeenCalledWith(true)
    expect(host.getClientMixStatus('client-a')).toMatchObject({
      globallyMuted: true,
      sttEnabled: true,
    })
  })

  it('listener mute does not call setSttEnabled', async () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    const agentA = fakeAgent()
    const agentB = fakeAgent()
    host.sessions.set('client-a', { agent: agentA, agentStarted: true })
    host.sessions.set('client-b', { agent: agentB, agentStarted: true })
    host.createMixGroup({ id: 'g1', clientIds: ['client-a', 'client-b'] })

    await host.setListenerMute('client-b', 'client-a', true)
    expect(graph.setListenerMute).toHaveBeenCalledWith('client-b', 'client-a', true)
    expect(agentA.setSttEnabled).not.toHaveBeenCalled()
    expect(agentB.setSttEnabled).not.toHaveBeenCalled()
  })

  it('listener mute throws when clients are not in the same group', async () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    host.createMixGroup({ id: 'g1', clientIds: ['client-a'] })
    await expect(host.setListenerMute('client-b', 'client-a', true)).rejects.toThrow(
      /same mix group/,
    )
  })

  it('createMixGroup resolves orchestrator session ids to peer ids', () => {
    const graph = createMockMixGraph()
    const orchestratorSessionId = 'orch-session-listener'
    const host = new VoiceAgentSessionHost(
      createStubSignaling(orchestratorSessionId) as never,
      [],
      {
        voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } } as never,
        sessionMode: 'voice+data',
        clientMixGraph: graph,
        sessionBudget: {
          tryAcquire: () => 'lease-test',
          release: () => undefined,
          snapshot: () => ({ active: 0, max: 0, available: 0, rejectedTotal: 0 }),
        },
        resolveParticipantId: (id) => (id === orchestratorSessionId ? 'client-b' : id),
      },
    )
    host.createMixGroup({
      id: 'all',
      clientIds: ['client-a', orchestratorSessionId, 'client-c'],
    })
    expect(graph.setGroupMembers).toHaveBeenCalledWith('all', ['client-a', 'client-b', 'client-c'])
  })

  it('status reports pose, tts pose, and mutes', async () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    const pose = {
      position: { x: 2, y: 0, z: 0 },
      orientation: { x: 0, y: 0, z: 0, w: 1 },
    }
    const ttsPose = {
      position: { x: -2, y: 0, z: 0 },
      orientation: { x: 0, y: 0, z: 0, w: 1 },
    }
    graph.pose = vi.fn(() => pose)
    graph.ttsPose = vi.fn(() => ttsPose)
    host.createMixGroup({ id: 'g1', clientIds: ['client-a', 'client-b'] })
    host.setClientPose('client-a', pose)
    await host.setTtsPose('client-a', ttsPose)
    await host.setListenerMute('client-b', 'client-a', true)

    const status = host.getClientMixStatus('client-a')
    expect(status.pose).toEqual(pose)
    expect(status.ttsPose).toEqual(ttsPose)
    expect(status.mutedBy).toEqual(['client-b'])
    expect(status.groupId).toBe('g1')
  })
})
