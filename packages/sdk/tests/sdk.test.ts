import { describe, expect, test } from 'vitest'

import { LocalAudioTrack, RTCPeerConnection, RTCSessionDescription } from '../src'

describe('SDK type surface', () => {
  test('RTCSessionDescription serializes to JSON', () => {
    const desc = new RTCSessionDescription({ type: 'offer', sdp: 'v=0' })
    expect(desc.toJSON()).toEqual({ type: 'offer', sdp: 'v=0' })
  })

  test('RTCPeerConnection exposes connection state getters', () => {
    const pc = new RTCPeerConnection()
    expect(pc.connectionState).toBeDefined()
    expect(pc.iceGatheringState).toBe('new')
    expect(pc.signalingState).toBe('stable')
    pc.close()
  })

  test('RTCPeerConnection fires negotiationneeded when adding a track', async () => {
    const pc = new RTCPeerConnection()
    const fired = new Promise<void>((resolve) => {
      pc.onnegotiationneeded = () => resolve()
    })

    const track = new LocalAudioTrack('a1', 's1')
    await pc.addTrack(track)

    await expect(fired).resolves.toBeUndefined()
    pc.close()
  })
})
