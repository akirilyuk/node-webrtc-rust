import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { ClientAudioMixer, sumStereoPcm, type ClientMixGraph } from '../src/client-audio-mixer.js'
import { PCM_FRAME_DURATION_MS, PCM_FULL_FRAME_BYTES } from '../src/pcm.js'

function createMockGraph(): ClientMixGraph & {
  calls: {
    pushFrame: Array<{ peer: string; len: number }>
    renderOutput: string[]
    panTtsFrame: number[]
    panTtsListenerIds: string[]
    setGroupMembers: Array<{ groupId: string; members: string[] }>
    moveToGroup: Array<{ peer: string; groupId: string }>
    removeFromGroup: string[]
    setPose: Array<{ peer: string; pose: unknown }>
    setPositionalEnabled: boolean[]
    setDefaultMixPlacement: string[]
    setTtsMixPlacement: string[]
    setTtsPose: Array<{ peer: string; pose: unknown }>
    clearTtsPose: string[]
    addInput: string[]
    removeInput: string[]
  }
} {
  const calls = {
    pushFrame: [] as Array<{ peer: string; len: number }>,
    renderOutput: [] as string[],
    panTtsFrame: [] as number[],
    panTtsListenerIds: [] as string[],
    setGroupMembers: [] as Array<{ groupId: string; members: string[] }>,
    moveToGroup: [] as Array<{ peer: string; groupId: string }>,
    removeFromGroup: [] as string[],
    setGlobalMute: [] as Array<{ target: string; muted: boolean }>,
    setListenerMute: [] as Array<{ listener: string; target: string; muted: boolean }>,
    setPose: [] as Array<{ peer: string; pose: unknown }>,
    setPositionalEnabled: [] as boolean[],
    setDefaultMixPlacement: [] as string[],
    setTtsMixPlacement: [] as string[],
    setTtsPose: [] as Array<{ peer: string; pose: unknown }>,
    clearTtsPose: [] as string[],
    addInput: [] as string[],
    removeInput: [] as string[],
  }

  const silence = Buffer.alloc(PCM_FULL_FRAME_BYTES)

  const graph: ClientMixGraph = {
    addInput: (id) => {
      calls.addInput.push(id)
    },
    removeInput: (id) => {
      calls.removeInput.push(id)
    },
    pushFrame: (peer, pcm) => {
      calls.pushFrame.push({ peer, len: pcm.length })
    },
    renderOutput: (listenerId) => {
      calls.renderOutput.push(listenerId)
      return Buffer.from(silence)
    },
    panTtsFrame: (pcm, listenerId) => {
      calls.panTtsFrame.push(pcm.length)
      calls.panTtsListenerIds.push(listenerId)
      return Buffer.from(pcm)
    },
    setPose: (peer, pose) => {
      calls.setPose.push({ peer, pose })
    },
    setPositionalEnabled: (enabled) => {
      calls.setPositionalEnabled.push(enabled)
    },
    setDefaultMixPlacement: (placement) => {
      calls.setDefaultMixPlacement.push(placement)
    },
    setTtsMixPlacement: (placement) => {
      calls.setTtsMixPlacement.push(placement)
    },
    setTtsPose: (peer, pose) => {
      calls.setTtsPose.push({ peer, pose })
    },
    clearTtsPose: (peer) => {
      calls.clearTtsPose.push(peer)
    },
    setGroupMembers: (groupId, members) => {
      calls.setGroupMembers.push({ groupId, members })
    },
    moveToGroup: (peer, groupId) => {
      calls.moveToGroup.push({ peer, groupId })
    },
    removeFromGroup: (peer) => {
      calls.removeFromGroup.push(peer)
    },
    setGlobalMute: (target, muted) => {
      calls.setGlobalMute.push({ target, muted })
    },
    isGloballyMuted: (target) =>
      calls.setGlobalMute.some((entry) => entry.target === target && entry.muted),
    setListenerMute: (listener, target, muted) => {
      calls.setListenerMute.push({ listener, target, muted })
    },
    isListenerMuted: (listener, target) =>
      calls.setListenerMute.some(
        (entry) => entry.listener === listener && entry.target === target && entry.muted,
      ),
    pose: () => null,
    ttsPose: () => null,
  }

  return Object.assign(graph, { calls })
}

function createFakeSidecar(): {
  setWriteSampleTee: ReturnType<typeof vi.fn>
  tee?: (...args: unknown[]) => void
} {
  let tee: ((...args: unknown[]) => void) | undefined
  return {
    setWriteSampleTee: vi.fn((cb: ((...args: unknown[]) => void) | null) => {
      tee = cb ?? undefined
    }),
    tee(...args: unknown[]) {
      tee?.(...args)
    },
  }
}

describe('sumStereoPcm', () => {
  it('adds int16 samples with saturation', () => {
    const a = Buffer.alloc(4)
    const b = Buffer.alloc(4)
    a.writeInt16LE(30_000, 0)
    a.writeInt16LE(-30_000, 2)
    b.writeInt16LE(10_000, 0)
    b.writeInt16LE(-10_000, 2)
    const out = sumStereoPcm(a, b)
    expect(out.readInt16LE(0)).toBe(32_767)
    expect(out.readInt16LE(2)).toBe(-32_768)
  })
})

describe('ClientAudioMixer', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('registers peers and tees inbound PCM into the graph', async () => {
    const graph = createMockGraph()
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('alice')

    const pcm = Buffer.alloc(PCM_FULL_FRAME_BYTES, 1)
    const track = {
      readSample: vi.fn(async () => pcm),
    }
    mixer.wrapInboundTrack('alice', track as never)
    const read = await track.readSample()

    expect(read).toBe(pcm)
    expect(graph.calls.addInput).toEqual(['alice'])
    expect(graph.calls.pushFrame).toEqual([{ peer: 'alice', len: PCM_FULL_FRAME_BYTES }])
  })

  it('buffers partial inbound reads until a full mix frame is available', async () => {
    const graph = createMockGraph()
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('alice')

    const halfFrameBytes = PCM_FULL_FRAME_BYTES / 2
    const chunkA = Buffer.alloc(halfFrameBytes, 1)
    const chunkB = Buffer.alloc(halfFrameBytes, 2)
    let readCount = 0
    const track = {
      readSample: vi.fn(async () => {
        readCount += 1
        if (readCount === 1) return chunkA
        if (readCount === 2) return chunkB
        if (readCount === 3) return Buffer.alloc(100, 3)
        return Buffer.alloc(PCM_FULL_FRAME_BYTES - 100, 4)
      }),
    }
    mixer.wrapInboundTrack('alice', track as never)

    await track.readSample()
    expect(graph.calls.pushFrame).toEqual([])

    await track.readSample()
    expect(graph.calls.pushFrame).toEqual([{ peer: 'alice', len: PCM_FULL_FRAME_BYTES }])

    graph.calls.pushFrame.length = 0

    await track.readSample()
    expect(graph.calls.pushFrame).toEqual([])

    await track.readSample()
    expect(graph.calls.pushFrame).toEqual([{ peer: 'alice', len: PCM_FULL_FRAME_BYTES }])
  })

  it('mix pump calls renderOutput and writes PC track without TTS', async () => {
    const graph = createMockGraph()
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('bob')

    const pcTrack = { writeSample: vi.fn(async () => undefined) }
    mixer.startMixPump('bob', pcTrack)

    await vi.advanceTimersByTimeAsync(PCM_FRAME_DURATION_MS)

    expect(graph.calls.renderOutput).toEqual(['bob'])
    expect(graph.calls.panTtsFrame).toEqual([PCM_FULL_FRAME_BYTES])
    expect(graph.calls.panTtsListenerIds).toEqual(['bob'])
    expect(pcTrack.writeSample).toHaveBeenCalledTimes(1)
    expect(pcTrack.writeSample).toHaveBeenCalledWith(expect.any(Buffer), PCM_FRAME_DURATION_MS)
  })

  it('sidecar tee TTS is passed to panTtsFrame and summed on the pump tick', async () => {
    const graph = createMockGraph()
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('carol')

    const sidecar = createFakeSidecar()
    mixer.wireTtsSidecar('carol', sidecar)

    const pcTrack = { writeSample: vi.fn(async () => undefined) }

    const tts = Buffer.alloc(PCM_FULL_FRAME_BYTES, 7)
    sidecar.tee!(tts, PCM_FRAME_DURATION_MS)
    await mixer.pumpMixFrame('carol', pcTrack)

    expect(graph.calls.panTtsFrame).toEqual([PCM_FULL_FRAME_BYTES])
    expect(graph.calls.renderOutput).toEqual(['carol'])
    expect(pcTrack.writeSample).toHaveBeenCalledTimes(1)
  })

  it('sidecar tee assembles two half frames into one pending TTS frame', async () => {
    const panInputs: Buffer[] = []
    const graph = createMockGraph()
    graph.panTtsFrame = (pcm, listenerId) => {
      panInputs.push(Buffer.from(pcm))
      graph.calls.panTtsListenerIds.push(listenerId)
      return Buffer.from(pcm)
    }
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('frank')

    const sidecar = createFakeSidecar()
    mixer.wireTtsSidecar('frank', sidecar)

    const pcTrack = { writeSample: vi.fn(async () => undefined) }

    const half = PCM_FULL_FRAME_BYTES / 2
    const chunkA = Buffer.alloc(half, 11)
    const chunkB = Buffer.alloc(half, 22)
    sidecar.tee!(chunkA, PCM_FRAME_DURATION_MS)
    sidecar.tee!(chunkB, PCM_FRAME_DURATION_MS)
    await mixer.pumpMixFrame('frank', pcTrack)

    expect(panInputs).toHaveLength(1)
    expect(panInputs[0]!.subarray(0, half).equals(chunkA)).toBe(true)
    expect(panInputs[0]!.subarray(half).equals(chunkB)).toBe(true)
  })

  it('consumes TTS once per tee — next pump tick uses silence', async () => {
    const panInputs: Buffer[] = []
    const graph = createMockGraph()
    graph.panTtsFrame = (pcm, listenerId) => {
      panInputs.push(Buffer.from(pcm))
      graph.calls.panTtsListenerIds.push(listenerId)
      return Buffer.from(pcm)
    }
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('eve')

    const sidecar = createFakeSidecar()
    mixer.wireTtsSidecar('eve', sidecar)

    const pcTrack = { writeSample: vi.fn(async () => undefined) }

    const tts = Buffer.alloc(PCM_FULL_FRAME_BYTES, 9)
    sidecar.tee!(tts, PCM_FRAME_DURATION_MS)
    await mixer.pumpMixFrame('eve', pcTrack)

    expect(panInputs).toHaveLength(1)
    expect(panInputs[0]!.equals(tts)).toBe(true)

    await mixer.pumpMixFrame('eve', pcTrack)
    expect(panInputs).toHaveLength(2)
    expect(panInputs[1]!.every((byte) => byte === 0)).toBe(true)
  })

  it('unregisterPeer stops the mix pump', async () => {
    const graph = createMockGraph()
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('dana')

    const pcTrack = { writeSample: vi.fn(async () => undefined) }
    mixer.startMixPump('dana', pcTrack)

    await vi.advanceTimersByTimeAsync(PCM_FRAME_DURATION_MS)
    expect(pcTrack.writeSample).toHaveBeenCalledTimes(1)

    mixer.unregisterPeer('dana')
    graph.calls.renderOutput.length = 0
    pcTrack.writeSample.mockClear()

    await vi.advanceTimersByTimeAsync(PCM_FRAME_DURATION_MS * 3)
    expect(pcTrack.writeSample).not.toHaveBeenCalled()
    expect(graph.calls.renderOutput).toHaveLength(0)
  })

  it('unregisters peer from graph on teardown', () => {
    const graph = createMockGraph()
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('x')
    mixer.unregisterPeer('x')
    expect(graph.calls.removeInput).toEqual(['x'])
    expect(graph.calls.removeFromGroup).toEqual(['x'])
  })

  it('forwards group and placement controls', async () => {
    const graph = createMockGraph()
    const mixer = new ClientAudioMixer({ graph })

    mixer.setGroupMembers('g1', ['a', 'b'])
    mixer.moveToGroup('c', 'g1')
    mixer.removeFromGroup('c')
    mixer.setPositionalEnabled(false)
    mixer.setDefaultMixPlacement('left')
    mixer.setTtsMixPlacement('right')
    const pose = {
      position: { x: 1, y: 0, z: 0 },
      orientation: { x: 0, y: 0, z: 0, w: 1 },
    }
    await mixer.setTtsPose('peer-1', pose)
    await mixer.clearTtsPose('peer-1')

    expect(graph.calls.setGroupMembers).toEqual([{ groupId: 'g1', members: ['a', 'b'] }])
    expect(graph.calls.moveToGroup).toEqual([{ peer: 'c', groupId: 'g1' }])
    expect(graph.calls.removeFromGroup).toEqual(['c'])
    expect(graph.calls.setPositionalEnabled).toEqual([false])
    expect(graph.calls.setDefaultMixPlacement).toEqual(['left'])
    expect(graph.calls.setTtsMixPlacement).toEqual(['right'])
    expect(graph.calls.setTtsPose).toEqual([{ peer: 'peer-1', pose }])
    expect(graph.calls.clearTtsPose).toEqual(['peer-1'])
  })

  it('flushes outbound mix when TTS pose changes', async () => {
    const graph = createMockGraph()
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('peer-1')
    const pcTrack = { writeSample: vi.fn(async () => undefined) }
    mixer.startMixPump('peer-1', pcTrack)
    const pose = {
      position: { x: 1, y: 0, z: 0 },
      orientation: { x: 0, y: 0, z: 0, w: 1 },
    }
    const renderBefore = graph.calls.renderOutput.length
    const flushPromise = mixer.setTtsPose('peer-1', pose)
    await vi.advanceTimersByTimeAsync(25 * PCM_FRAME_DURATION_MS)
    await flushPromise
    expect(graph.calls.setTtsPose).toEqual([{ peer: 'peer-1', pose }])
    expect(graph.calls.renderOutput.length).toBeGreaterThan(renderBefore)
    expect(pcTrack.writeSample).toHaveBeenCalled()
  })

  it('forwards global mute to the graph and flushes post-mute mix', async () => {
    const graph = createMockGraph()
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('a')
    mixer.registerPeer('b')
    const pcTrack = { writeSample: vi.fn(async () => undefined) }
    mixer.startMixPump('b', pcTrack)
    await mixer.setGlobalMute('a', true)
    expect(graph.calls.setGlobalMute).toEqual([{ target: 'a', muted: true }])
    expect(graph.calls.renderOutput.length).toBeGreaterThan(0)
    expect(pcTrack.writeSample).toHaveBeenCalled()
  })

  it('does not push inbound PCM while globally muted', async () => {
    const graph = createMockGraph()
    graph.isGloballyMuted = (target) => target === 'a'
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('a')

    const pcm = Buffer.alloc(PCM_FULL_FRAME_BYTES, 1)
    const track = { readSample: vi.fn(async () => pcm) }
    mixer.wrapInboundTrack('a', track as never)
    await track.readSample()

    expect(graph.calls.pushFrame).toEqual([])
  })

  it('tracks mix groups and rejects listener mute across groups', async () => {
    const graph = createMockGraph()
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('a')
    mixer.registerPeer('b')
    mixer.registerPeer('c')
    mixer.setGroupMembers('g1', ['a', 'b'])

    await mixer.setListenerMute('a', 'b', true)
    expect(graph.calls.setListenerMute).toEqual([{ listener: 'a', target: 'b', muted: true }])

    await expect(mixer.setListenerMute('a', 'c', true)).rejects.toThrow(/same mix group/)
    await expect(mixer.setListenerMute('a', 'a', true)).rejects.toThrow(/self/)
  })

  it('listRegisteredPeers is graph-scoped across mixer instances', () => {
    const graph = createMockGraph()
    const mixer1 = new ClientAudioMixer({ graph })
    const mixer2 = new ClientAudioMixer({ graph })
    mixer1.registerPeer('a')
    mixer2.registerPeer('b')
    expect(mixer1.listRegisteredPeers()).toEqual(['a', 'b'])
    expect(mixer2.listRegisteredPeers()).toEqual(['a', 'b'])
  })

  it('reports mix snapshot with group and listener mutes', async () => {
    const graph = createMockGraph()
    const pose = {
      position: { x: 1, y: 0, z: 0 },
      orientation: { x: 0, y: 0, z: 0, w: 1 },
    }
    graph.pose = (peer) => (peer === 'a' ? pose : null)
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('a')
    mixer.registerPeer('b')
    mixer.setGroupMembers('team', ['a', 'b'])
    await mixer.setGlobalMute('a', true)
    await mixer.setListenerMute('b', 'a', true)

    const status = mixer.getMixSnapshot('a')
    expect(status.globallyMuted).toBe(true)
    expect(status.groupId).toBe('team')
    expect(status.pose).toEqual(pose)
    expect(status.mutedBy).toEqual(['b'])
  })
})
