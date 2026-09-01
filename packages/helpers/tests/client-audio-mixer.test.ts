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
    setTtsPose: unknown[]
    clearTtsPose: number[]
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
    setPose: [] as Array<{ peer: string; pose: unknown }>,
    setPositionalEnabled: [] as boolean[],
    setDefaultMixPlacement: [] as string[],
    setTtsMixPlacement: [] as string[],
    setTtsPose: [] as unknown[],
    clearTtsPose: [] as number[],
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
    setTtsPose: (pose) => {
      calls.setTtsPose.push(pose)
    },
    clearTtsPose: () => {
      calls.clearTtsPose.push(1)
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
    mixer.startMixPump('carol', pcTrack)

    const tts = Buffer.alloc(PCM_FULL_FRAME_BYTES, 7)
    sidecar.tee!(tts, PCM_FRAME_DURATION_MS)

    await vi.advanceTimersByTimeAsync(PCM_FRAME_DURATION_MS)

    expect(graph.calls.panTtsFrame).toEqual([PCM_FULL_FRAME_BYTES])
    expect(graph.calls.renderOutput).toEqual(['carol'])
    expect(pcTrack.writeSample).toHaveBeenCalledTimes(1)
  })

  it('consumes TTS once per tee — next pump tick uses silence', async () => {
    const panInputs: Buffer[] = []
    const graph = createMockGraph()
    graph.panTtsFrame = (pcm, listenerId) => {
      panInputs.push(Buffer.from(pcm))
      calls.panTtsListenerIds.push(listenerId)
      return Buffer.from(pcm)
    }
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('eve')

    const sidecar = createFakeSidecar()
    mixer.wireTtsSidecar('eve', sidecar)

    const pcTrack = { writeSample: vi.fn(async () => undefined) }
    mixer.startMixPump('eve', pcTrack)

    const tts = Buffer.alloc(PCM_FULL_FRAME_BYTES, 9)
    sidecar.tee!(tts, PCM_FRAME_DURATION_MS)

    await vi.advanceTimersByTimeAsync(PCM_FRAME_DURATION_MS)
    expect(panInputs).toHaveLength(1)
    expect(panInputs[0]!.equals(tts)).toBe(true)

    await vi.advanceTimersByTimeAsync(PCM_FRAME_DURATION_MS)
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

  it('forwards group and placement controls', () => {
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
    mixer.setTtsPose(pose)
    mixer.clearTtsPose()

    expect(graph.calls.setGroupMembers).toEqual([{ groupId: 'g1', members: ['a', 'b'] }])
    expect(graph.calls.moveToGroup).toEqual([{ peer: 'c', groupId: 'g1' }])
    expect(graph.calls.removeFromGroup).toEqual(['c'])
    expect(graph.calls.setPositionalEnabled).toEqual([false])
    expect(graph.calls.setDefaultMixPlacement).toEqual(['left'])
    expect(graph.calls.setTtsMixPlacement).toEqual(['right'])
    expect(graph.calls.setTtsPose).toEqual([pose])
    expect(graph.calls.clearTtsPose).toEqual([1])
  })
})
