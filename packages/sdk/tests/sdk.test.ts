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

  test('RTCRtpSender.replaceTrack updates sender track reference', async () => {
    const pc = new RTCPeerConnection()
    const trackA = new LocalAudioTrack('a1', 's1')
    const trackB = new LocalAudioTrack('a2', 's1')
    const sender = await pc.addTrack(trackA)
    expect(sender.track?.id).toBe('a1')
    await sender.replaceTrack(trackB)
    expect(sender.track?.id).toBe('a2')
    await sender.replaceTrack(null)
    expect(sender.track).toBeNull()
    pc.close()
  })

  test('createOffer accepts offerToReceiveAudio option', async () => {
    const pc = new RTCPeerConnection()
    const desc = await pc.createOffer({ offerToReceiveAudio: true })
    expect(desc.sdp).toContain('m=audio')
    pc.close()
  })

  test('RTCPeerConnection.removeTrack accepts RTCRtpSender from addTrack', async () => {
    const pc = new RTCPeerConnection()
    const track = new LocalAudioTrack('a1', 's1')
    const sender = await pc.addTrack(track)
    await expect(pc.removeTrack(sender)).resolves.toBeUndefined()
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
