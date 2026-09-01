import { describe, expect, it, vi } from 'vitest'

import { ClientAudioMixer, sumStereoPcm, type ClientMixGraph } from '../src/client-audio-mixer.js'
import { PCM_FULL_FRAME_BYTES } from '../src/pcm.js'

function createMockGraph(): ClientMixGraph & {
  calls: {
    pushFrame: Array<{ peer: string; len: number }>
    renderOutput: string[]
    panTtsFrame: number[]
    setGroupMembers: Array<{ groupId: string; members: string[] }>
    moveToGroup: Array<{ peer: string; groupId: string }>
    removeFromGroup: string[]
    setPose: Array<{ peer: string; pose: unknown }>
    setPositionalEnabled: boolean[]
    setDefaultMixPlacement: string[]
    setTtsMixPlacement: string[]
    addInput: string[]
    removeInput: string[]
  }
} {
  const calls = {
    pushFrame: [] as Array<{ peer: string; len: number }>,
    renderOutput: [] as string[],
    panTtsFrame: [] as number[],
    setGroupMembers: [] as Array<{ groupId: string; members: string[] }>,
    moveToGroup: [] as Array<{ peer: string; groupId: string }>,
    removeFromGroup: [] as string[],
    setPose: [] as Array<{ peer: string; pose: unknown }>,
    setPositionalEnabled: [] as boolean[],
    setDefaultMixPlacement: [] as string[],
    setTtsMixPlacement: [] as string[],
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
    panTtsFrame: (pcm) => {
      calls.panTtsFrame.push(pcm.length)
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

  it('pans TTS, renders mix, and sums on outbound writeSample', async () => {
    const graph = createMockGraph()
    const mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('bob')

    const tts = Buffer.alloc(PCM_FULL_FRAME_BYTES, 2)
    const written: Array<{ len: number; durationMs: number }> = []
    const track = {
      writeSample: vi.fn(async (data: Buffer, durationMs: number) => {
        written.push({ len: data.length, durationMs })
      }),
    }
    mixer.wrapOutboundTrack('bob', track as never)
    await track.writeSample(tts, 20)

    expect(graph.calls.panTtsFrame).toEqual([PCM_FULL_FRAME_BYTES])
    expect(graph.calls.renderOutput).toEqual(['bob'])
    expect(written).toEqual([{ len: PCM_FULL_FRAME_BYTES, durationMs: 20 }])
  })

  it('passes through non-20ms kick frames on outbound', async () => {
    const graph = createMockGraph()
    const mixer = new ClientAudioMixer({ graph })
    const kick = Buffer.alloc(960)
    const track = {
      writeSample: vi.fn(async () => undefined),
    }
    mixer.wrapOutboundTrack('c', track as never)
    await track.writeSample(kick, 5)
    expect(graph.calls.panTtsFrame).toHaveLength(0)
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
})
