import { describe, expect, test } from 'vitest'

import { AudioMixGraph, MIX_PLACEMENT, quatIdentity, vec3Zero } from '../src/mix'

describe('AudioMixGraph', () => {
  test('positional flag and placements round-trip', () => {
    const graph = new AudioMixGraph()

    expect(graph.mixingEnabled()).toBe(true)
    expect(graph.positionalEnabled()).toBe(false)
    expect(graph.defaultMixPlacement()).toBe('center')
    expect(graph.ttsMixPlacement()).toBe('center')

    graph.setPositionalEnabled(true)
    graph.setDefaultMixPlacement(MIX_PLACEMENT.Left)
    graph.setTtsMixPlacement(MIX_PLACEMENT.Right)

    expect(graph.positionalEnabled()).toBe(true)
    expect(graph.defaultMixPlacement()).toBe('left')
    expect(graph.ttsMixPlacement()).toBe('right')
  })

  test('listener sources and pose control', () => {
    const graph = new AudioMixGraph()
    graph.addInput('alice')
    graph.addInput('bob')

    graph.setListenerSources('alice', ['bob'])
    expect(graph.listenerSources('alice')).toEqual(['bob'])

    const pose = {
      position: { ...vec3Zero(), x: 2 },
      orientation: quatIdentity(),
    }
    graph.setPose('bob', pose)
    expect(graph.pose('bob')).toEqual(pose)

    graph.clearListenerRoutes('alice')
    expect(graph.listenerSources('alice')).toBeNull()
  })

  test('group membership updates listener routes', () => {
    const graph = new AudioMixGraph()
    graph.addInput('a')
    graph.addInput('b')
    graph.addInput('c')

    graph.setGroupMembers('team', ['a', 'b'])
    expect(graph.listenerSources('a')?.sort()).toEqual(['b'])
    expect(graph.listenerSources('b')?.sort()).toEqual(['a'])

    graph.moveToGroup('c', 'team')
    expect(graph.listenerSources('c')?.sort()).toEqual(['a', 'b'])
  })

  test('distance params round-trip', () => {
    const graph = new AudioMixGraph()
    graph.setDistanceParams({
      referenceDistance: 1.5,
      maxDistance: 40,
      rolloff: 0.8,
    })
    const params = graph.distanceParams()
    expect(params.referenceDistance).toBeCloseTo(1.5)
    expect(params.maxDistance).toBeCloseTo(40)
    expect(params.rolloff).toBeCloseTo(0.8)
  })

  test('pushFrame renderOutput and panTtsFrame on empty graph', () => {
    const graph = new AudioMixGraph()
    const silence = Buffer.alloc(3840)
    graph.pushFrame('alice', silence)
    const mixed = graph.renderOutput('bob')
    expect(mixed).toHaveLength(3840)
    const panned = graph.panTtsFrame(silence, 'bob')
    expect(panned).toHaveLength(3840)
  })

  test('setSourceMixPlacement pans source left when positional off', async () => {
    const graph = new AudioMixGraph()
    graph.addInput('listener')
    graph.addInput('source')
    graph.setSourceMixPlacement('source', MIX_PLACEMENT.Left)
    const mono = Buffer.alloc(3840)
    for (let i = 0; i < 3840; i += 4) {
      mono.writeInt16LE(10_000, i)
      mono.writeInt16LE(10_000, i + 2)
    }
    graph.pushFrame('source', mono)
    graph.setListenerSources('listener', ['source'])
    // A lone frame plays once it has waited out the two-tick prefill.
    await new Promise((resolve) => setTimeout(resolve, 50))
    const out = graph.renderOutput('listener')
    const l = out.readInt16LE(0)
    const r = out.readInt16LE(2)
    expect(l).toBeGreaterThan(r)
  })

  test('setTtsPose pans TTS right vs center when positional on', () => {
    const graph = new AudioMixGraph()
    graph.setPositionalEnabled(true)
    graph.setTtsPose('listener', {
      position: { ...vec3Zero(), x: 3 },
      orientation: quatIdentity(),
    })
    const mono = Buffer.alloc(3840)
    for (let i = 0; i < 3840; i += 4) {
      mono.writeInt16LE(10_000, i)
      mono.writeInt16LE(10_000, i + 2)
    }
    const center = graph.panTtsFrame(mono, 'listener')
    const lCenter = center.readInt16LE(0)
    const rCenter = center.readInt16LE(2)
    expect(rCenter).toBeGreaterThan(lCenter)
    graph.clearTtsPose('listener')
    graph.setTtsMixPlacement(MIX_PLACEMENT.Center)
    const named = graph.panTtsFrame(mono, 'listener')
    const lNamed = named.readInt16LE(0)
    const rNamed = named.readInt16LE(2)
    expect(Math.abs(lNamed - rNamed)).toBeLessThan(500)
  })

  test('setTtsPose flip from +x to -x repans dual-mono without stale right dominance', () => {
    const graph = new AudioMixGraph()
    graph.setPositionalEnabled(true)
    const dualMono = Buffer.alloc(3840)
    for (let i = 0; i < 3840; i += 4) {
      dualMono.writeInt16LE(12_000, i)
      dualMono.writeInt16LE(12_000, i + 2)
    }
    graph.setTtsPose('listener', {
      position: { ...vec3Zero(), x: 3 },
      orientation: quatIdentity(),
    })
    const rightPanned = graph.panTtsFrame(dualMono, 'listener')
    const lRight = rightPanned.readInt16LE(0)
    const rRight = rightPanned.readInt16LE(2)
    expect(rRight).toBeGreaterThan(lRight * 1.5)

    graph.setTtsPose('listener', {
      position: { ...vec3Zero(), x: -3 },
      orientation: quatIdentity(),
    })
    const leftPanned = graph.panTtsFrame(dualMono, 'listener')
    const lLeft = leftPanned.readInt16LE(0)
    const rLeft = leftPanned.readInt16LE(2)
    expect(lLeft).toBeGreaterThan(rLeft * 1.5)
  })

  test('setTtsPose pans left-only TTS to the right channel', () => {
    const graph = new AudioMixGraph()
    graph.setPositionalEnabled(true)
    graph.setTtsPose('listener', {
      position: { ...vec3Zero(), x: 3 },
      orientation: quatIdentity(),
    })
    const leftOnly = Buffer.alloc(3840)
    for (let i = 0; i < 3840; i += 4) {
      leftOnly.writeInt16LE(10_000, i)
      leftOnly.writeInt16LE(0, i + 2)
    }
    const panned = graph.panTtsFrame(leftOnly, 'listener')
    const l = panned.readInt16LE(0)
    const r = panned.readInt16LE(2)
    expect(r).toBeGreaterThan(l * 1.5)
    expect(r).toBeGreaterThan(0)
  })

  test('mixStats reports queue counters for a source and null for an unknown one', () => {
    const graph = new AudioMixGraph()
    expect(graph.mixStats('nobody')).toBeNull()

    graph.addInput('alice')
    expect(graph.mixStats('alice')).toEqual({
      pushed: 0,
      played: 0,
      compressed: 0,
      concealed: 0,
      overflowCompressed: 0,
      decodeErrors: 0,
      listenerCatchupCompressed: 0,
      listenerSkipped: 0,
      depth: 0,
      target: 2,
    })

    const frame = Buffer.alloc(3840)
    for (let i = 0; i < 5; i++) {
      graph.pushFrame('alice', frame)
    }
    const stats = graph.mixStats('alice')
    expect(stats).toMatchObject({ pushed: 5, played: 0, depth: 5 })
    expect(stats!.pushed).toBe(
      stats!.played + stats!.compressed + stats!.overflowCompressed + stats!.depth,
    )
  })

  test('bunched pushFrame calls are queued, not overwritten', async () => {
    const graph = new AudioMixGraph()
    graph.addInput('talker')
    graph.setListenerSources('listener', ['talker'])
    // Ten frames in one instant, value = 1000 * (i + 1).
    for (let i = 0; i < 10; i++) {
      const frame = Buffer.alloc(3840)
      for (let n = 0; n < 3840; n += 2) {
        frame.writeInt16LE(1000 * (i + 1), n)
      }
      graph.pushFrame('talker', frame)
    }
    const heard: number[] = []
    const deadline = Date.now() + 2_000
    while (heard.length < 10 && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 20))
      const out = graph.renderOutput('listener')
      const sample = out.readInt16LE(0)
      if (sample !== 0 && heard[heard.length - 1] !== sample) {
        heard.push(sample)
      }
    }
    // Centre pan scales every frame equally, so the heard values keep their order.
    expect(heard).toHaveLength(10)
    expect([...heard].sort((a, b) => a - b)).toEqual(heard)
    expect(graph.mixStats('talker')).toMatchObject({ pushed: 10, compressed: 0 })
  })

  describe('renderOutput at a scheduled time', () => {
    const constantFrame = (value: number): Buffer => {
      const frame = Buffer.alloc(3840)
      for (let n = 0; n < 3840; n += 2) {
        frame.writeInt16LE(value, n)
      }
      return frame
    }

    /** Graph with the clock started by a warm-up render at `atMs = 1000` and 10 queued frames. */
    const graphWithTenFrames = (): AudioMixGraph => {
      const graph = new AudioMixGraph()
      graph.addInput('talker')
      graph.setListenerSources('listener', ['talker'])
      graph.renderOutput('listener', 1000)
      for (let i = 0; i < 10; i++) {
        graph.pushFrame('talker', constantFrame(1000 * (i + 1)))
      }
      return graph
    }

    test('the same atMs twice returns the identical frame and plays one source frame', () => {
      const graph = graphWithTenFrames()
      const first = graph.renderOutput('listener', 1020)
      const again = graph.renderOutput('listener', 1020)
      expect(again.equals(first)).toBe(true)
      expect(first.readInt16LE(0)).not.toBe(0)
      expect(graph.mixStats('talker')).toMatchObject({ played: 1 })
    })

    test('atMs and atMs + 20 return consecutive frames', () => {
      const graph = graphWithTenFrames()
      const heard = [1020, 1040, 1060, 1080].map((atMs) =>
        graph.renderOutput('listener', atMs).readInt16LE(0),
      )
      expect(new Set(heard).size).toBe(4)
      expect([...heard].sort((a, b) => a - b)).toEqual(heard)
    })

    test('a catch-up double render never repeats a frame', () => {
      const graph = graphWithTenFrames()
      const heard = [1020, 1040, 1060, 1060 + 20, 1100].map((atMs) =>
        graph.renderOutput('listener', atMs).readInt16LE(0),
      )
      expect(new Set(heard).size).toBe(heard.length)
    })

    test('an atMs earlier than the first one does not advance the clock', () => {
      const graph = graphWithTenFrames()
      const first = graph.renderOutput('listener', 1020)
      const earlier = graph.renderOutput('listener', 10)
      expect(earlier.equals(first)).toBe(true)
    })

    test('non-finite atMs is ignored rather than throwing', () => {
      const graph = graphWithTenFrames()
      expect(graph.renderOutput('listener', Number.NaN)).toHaveLength(3840)
      expect(graph.renderOutput('listener', Number.POSITIVE_INFINITY)).toHaveLength(3840)
    })
  })
})
