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
    const panned = graph.panTtsFrame(silence)
    expect(panned).toHaveLength(3840)
  })
})
