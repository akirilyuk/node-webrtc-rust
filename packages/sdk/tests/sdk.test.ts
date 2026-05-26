import { describe, expect, test } from 'vitest'

import { RTCPeerConnection, RTCSessionDescription } from '../src'

describe('SDK type surface', () => {
  test('RTCSessionDescription serializes to JSON', () => {
    const desc = new RTCSessionDescription({ type: 'offer', sdp: 'v=0' })
    expect(desc.toJSON()).toEqual({ type: 'offer', sdp: 'v=0' })
  })

  test('RTCPeerConnection exposes connection state getters', () => {
    const pc = new RTCPeerConnection()
    expect(pc.connectionState).toBeDefined()
    pc.close()
  })
})
