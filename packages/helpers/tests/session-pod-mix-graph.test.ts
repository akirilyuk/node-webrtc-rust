import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { SignalingServer } from '@node-webrtc-rust/signaling'

import type { ClientMixGraph } from '../src/client-audio-mixer.js'
import { PCM_FULL_FRAME_BYTES } from '../src/pcm.js'
import { SessionPod } from '../src/session-pod.js'
import * as hostModule from '../src/voice-agent-session-host.js'
import type {
  VoiceAgentSessionHost,
  VoiceAgentSessionHostOptions,
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
    setTtsPose: vi.fn(),
    clearTtsPose: vi.fn(),
    setGroupMembers: vi.fn(),
    moveToGroup: vi.fn(),
    removeFromGroup: vi.fn(),
  }
}

describe('SessionPod shared client mix graph', () => {
  let server: SignalingServer | undefined
  let hostOptions: VoiceAgentSessionHostOptions[] = []
  let RealHost: typeof VoiceAgentSessionHost
  let ctorSpy: ReturnType<typeof vi.spyOn> | undefined

  beforeEach(async () => {
    hostOptions = []
    RealHost = (await vi.importActual<typeof hostModule>('../src/voice-agent-session-host.js'))
      .VoiceAgentSessionHost
    ctorSpy = vi
      .spyOn(hostModule, 'VoiceAgentSessionHost')
      .mockImplementation((signaling, iceServers, options) => {
        hostOptions.push(options)
        return new RealHost(signaling, iceServers, options)
      })
  })

  afterEach(async () => {
    ctorSpy?.mockRestore()
    vi.restoreAllMocks()
    if (server) {
      await server.close().catch(() => undefined)
      server = undefined
    }
  })

  async function createPod(
    options: ConstructorParameters<typeof SessionPod>[1],
  ): Promise<SessionPod> {
    server = new SignalingServer({ pingIntervalMs: 0 })
    await server.listen(0)
    return new SessionPod(server, {
      signalingUrl: `ws://127.0.0.1:${server.port}/ws`,
      iceServers: [],
      voiceConfig: {} as never,
      teardownIdleSessions: false,
      ...options,
    })
  }

  it('passes the same injected clientMixGraph to every prepared host', async () => {
    const sharedGraph = createMockMixGraph()
    const pod = await createPod({
      sessionMode: 'voice+data',
      clientMixGraph: sharedGraph,
    })

    await pod.ensureSession('session-a')
    await pod.ensureSession('session-b')

    expect(hostOptions).toHaveLength(2)
    expect(hostOptions[0]?.clientMixGraph).toBe(sharedGraph)
    expect(hostOptions[1]?.clientMixGraph).toBe(sharedGraph)

    const hostA = new RealHost({ room: 'x', on: vi.fn() } as never, [], hostOptions[0]!)
    hostA.setPositionalMixing(true)
    hostA.createMixGroup({ id: 'team1', clientIds: ['client-a'] })

    expect(sharedGraph.setPositionalEnabled).toHaveBeenCalledWith(true)
    expect(sharedGraph.setGroupMembers).toHaveBeenCalledWith('team1', ['client-a'])

    await pod.close().catch(() => undefined)
    server = undefined
  })

  it('creates one pod-level graph when clientMixGraph is omitted', async () => {
    const pod = await createPod({ sessionMode: 'voice+data' })

    await pod.ensureSession('session-a')
    await pod.ensureSession('session-b')

    expect(hostOptions).toHaveLength(2)
    expect(hostOptions[0]?.clientMixGraph).toBeDefined()
    expect(hostOptions[1]?.clientMixGraph).toBe(hostOptions[0]?.clientMixGraph)

    await pod.close().catch(() => undefined)
    server = undefined
  })

  it('does not attach a mix graph on data-only pods', async () => {
    const pod = await createPod({ sessionMode: 'data-only' })

    await pod.ensureSession('session-a')

    expect(hostOptions).toHaveLength(1)
    expect(hostOptions[0]?.clientMixGraph).toBeUndefined()

    await pod.close().catch(() => undefined)
    server = undefined
  })

  it('does not auto-create a mix graph when sessionMode is omitted or voice', async () => {
    const podOmitted = await createPod({})
    await podOmitted.ensureSession('session-omitted')
    expect(hostOptions.at(-1)?.clientMixGraph).toBeUndefined()
    await podOmitted.close().catch(() => undefined)

    hostOptions = []
    const podVoice = await createPod({ sessionMode: 'voice' })
    await podVoice.ensureSession('session-voice')
    expect(hostOptions.at(-1)?.clientMixGraph).toBeUndefined()
    await podVoice.close().catch(() => undefined)

    server = undefined
  })
})
