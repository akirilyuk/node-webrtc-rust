import { describe, expect, test } from 'vitest'

import { AudioMixGraph, quatIdentity, vec3Zero } from '../src/mix'
import {
  assertTwoSinePanSides,
  extractChannel,
  FRAME_BYTES,
  FRAME_COUNT,
  sineStereoFrame,
  SAMPLES_PER_CHANNEL,
} from './mix-test-helpers'

function setupThreeClientGraph(c1X: number, c3X: number): AudioMixGraph {
  const graph = new AudioMixGraph()
  for (const id of ['c1', 'c2', 'c3']) {
    graph.addInput(id)
  }
  graph.setPositionalEnabled(true)
  graph.setPose('c2', { position: vec3Zero(), orientation: quatIdentity() })
  graph.setPose('c1', {
    position: { ...vec3Zero(), x: c1X },
    orientation: quatIdentity(),
  })
  graph.setPose('c3', {
    position: { ...vec3Zero(), x: c3X },
    orientation: quatIdentity(),
  })
  graph.setListenerSources('c2', ['c1', 'c3'])
  return graph
}

function renderListenerAccumulated(graph: AudioMixGraph): { left: Int16Array; right: Int16Array } {
  let phaseC1 = 0
  let phaseC3 = 0
  const left: number[] = []
  const right: number[] = []

  for (let frame = 0; frame < FRAME_COUNT; frame++) {
    graph.pushFrame('c1', sineStereoFrame(440, 10_000, phaseC1))
    graph.pushFrame('c3', sineStereoFrame(880, 10_000, phaseC3))
    phaseC1 += SAMPLES_PER_CHANNEL
    phaseC3 += SAMPLES_PER_CHANNEL

    const mixed = graph.renderOutput('c2')
    expect(mixed).toHaveLength(FRAME_BYTES)
    left.push(...extractChannel(mixed, 0))
    right.push(...extractChannel(mixed, 1))
  }

  return { left: Int16Array.from(left), right: Int16Array.from(right) }
}

describe('AudioMixGraph stereo positional pan', () => {
  test('two sine sources pan 440 Hz right and 880 Hz left for listener c2', () => {
    const graph = setupThreeClientGraph(3, -3)
    const { left, right } = renderListenerAccumulated(graph)
    assertTwoSinePanSides(left, right, true)
  })

  test('swapped source poses flip stereo sides for two sine sources', () => {
    const graph = setupThreeClientGraph(-3, 3)
    const { left, right } = renderListenerAccumulated(graph)
    assertTwoSinePanSides(left, right, false)
  })
})
