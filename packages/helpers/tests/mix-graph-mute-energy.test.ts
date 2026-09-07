import { describe, expect, it } from 'vitest'

import { AudioMixGraph, quatIdentity, vec3Zero } from '@node-webrtc-rust/sdk/mix'

import {
  assertToneAbsentStereo,
  assertTonePresentStereo,
  FRAME_BYTES,
  sineStereoFrame,
  SAMPLES_PER_CHANNEL,
} from './mix-three-client-helpers.js'

function mixGraphNativeAvailable(): boolean {
  try {
    const graph = new AudioMixGraph()
    graph.addInput('probe')
    graph.removeInput('probe')
    return true
  } catch {
    return false
  }
}

function poseAtX(x: number) {
  return {
    position: { ...vec3Zero(), x },
    orientation: quatIdentity(),
  }
}

describe.skipIf(!mixGraphNativeAvailable())('AudioMixGraph mute energy (native graph only)', () => {
  it('global mute excludes muted source from renderOutput', () => {
    const graph = new AudioMixGraph()
    for (const id of ['client-mix-1', 'client-mix-2', 'client-mix-3']) {
      graph.addInput(id)
    }
    graph.setGroupMembers('all', ['client-mix-1', 'client-mix-2', 'client-mix-3'])
    graph.setPositionalEnabled(true)
    graph.setPose('client-mix-2', poseAtX(0))
    graph.setPose('client-mix-1', poseAtX(3))
    graph.setPose('client-mix-3', poseAtX(-3))

    graph.setGlobalMute('client-mix-1', true)
    expect(graph.isGloballyMuted('client-mix-1')).toBe(true)

    const phaseRef = { value: 0 }
    graph.pushFrame('client-mix-1', sineStereoFrame(440, 10_000, phaseRef.value))
    phaseRef.value += SAMPLES_PER_CHANNEL

    const pcm = graph.renderOutput('client-mix-2')
    expect(pcm.length).toBe(FRAME_BYTES)

    const left: number[] = []
    const right: number[] = []
    for (let i = 0; i < SAMPLES_PER_CHANNEL; i++) {
      left.push(pcm.readInt16LE(i * 4))
      right.push(pcm.readInt16LE(i * 4 + 2))
    }
    assertToneAbsentStereo(
      Int16Array.from(left),
      Int16Array.from(right),
      440,
      'graph after global mute',
    )
  })

  it('listener mute excludes target only for that listener', () => {
    const graph = new AudioMixGraph()
    for (const id of ['client-mix-1', 'client-mix-2', 'client-mix-3']) {
      graph.addInput(id)
    }
    graph.setGroupMembers('all', ['client-mix-1', 'client-mix-2', 'client-mix-3'])

    graph.setListenerMute('client-mix-2', 'client-mix-1', true)
    expect(graph.isListenerMuted('client-mix-2', 'client-mix-1')).toBe(true)

    const phaseRef = { value: 0 }
    graph.pushFrame('client-mix-1', sineStereoFrame(440, 10_000, phaseRef.value))
    phaseRef.value += SAMPLES_PER_CHANNEL

    const mutedListener = graph.renderOutput('client-mix-2')
    const otherListener = graph.renderOutput('client-mix-3')

    const readStereo = (pcm: Buffer) => {
      const left: number[] = []
      const right: number[] = []
      for (let i = 0; i < SAMPLES_PER_CHANNEL; i++) {
        left.push(pcm.readInt16LE(i * 4))
        right.push(pcm.readInt16LE(i * 4 + 2))
      }
      return { left: Int16Array.from(left), right: Int16Array.from(right) }
    }

    const c2 = readStereo(mutedListener)
    const c3 = readStereo(otherListener)
    assertToneAbsentStereo(c2.left, c2.right, 440, 'graph listener mute')
    assertTonePresentStereo(c3.left, c3.right, 440, 'graph other listener')
  })
})
