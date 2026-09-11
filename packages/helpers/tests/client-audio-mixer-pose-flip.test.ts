import { describe, expect, it } from 'vitest'

import { AudioMixGraph, quatIdentity, vec3Zero } from '@node-webrtc-rust/sdk/mix'

import { ClientAudioMixer } from '../src/client-audio-mixer.js'
import {
  assertLeftLouder,
  assertRightLouder,
  createLoudLeftOnlyFrame,
  injectTtsSidecarPendingFrame,
  mergeStereoRms,
  stereoRms,
} from './mix-energy-helpers.js'

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

function peakRmsFromFrames(frames: Buffer[]): { left: number; right: number } {
  let peak = { left: 0, right: 0 }
  for (const frame of frames) {
    peak = mergeStereoRms(peak, stereoRms(frame))
  }
  return peak
}

describe.skipIf(!mixGraphNativeAvailable())(
  'ClientAudioMixer TTS pose flip (native graph, no WebRTC)',
  () => {
    it('repans dual-mono sidecar after +x then -x setTtsPose on pumped PCM', async () => {
      const graph = new AudioMixGraph()
      graph.setPositionalEnabled(true)
      const mixer = new ClientAudioMixer({ graph })
      const peerId = 'listener'
      mixer.registerPeer(peerId)
      mixer.setClientPose(peerId, { position: vec3Zero(), orientation: quatIdentity() })

      const captured: Buffer[] = []
      const outbound = {
        writeSample: async (data: Buffer) => {
          captured.push(Buffer.from(data))
        },
      }
      mixer.startMixPump(peerId, outbound)

      await mixer.setTtsPose(peerId, poseAtX(3))
      const rightPhaseStart = captured.length
      for (let i = 0; i < 10; i++) {
        injectTtsSidecarPendingFrame(mixer, peerId, createLoudLeftOnlyFrame())
        await mixer.pumpMixFrame(peerId, outbound)
      }
      const rightPeak = peakRmsFromFrames(captured.slice(rightPhaseStart))
      assertRightLouder(rightPeak.left, rightPeak.right)

      await mixer.setTtsPose(peerId, poseAtX(-3))
      const leftPhaseStart = captured.length
      for (let i = 0; i < 10; i++) {
        injectTtsSidecarPendingFrame(mixer, peerId, createLoudLeftOnlyFrame())
        await mixer.pumpMixFrame(peerId, outbound)
      }
      const leftPeak = peakRmsFromFrames(captured.slice(leftPhaseStart))
      assertLeftLouder(leftPeak.left, leftPeak.right)

      mixer.stopMixPump(peerId)
      expect(captured.length).toBeGreaterThan(leftPhaseStart)
    })
  },
)
