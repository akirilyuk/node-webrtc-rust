import { afterEach, describe, expect, it } from 'vitest'

import { parseIceTransportPolicy, resolveIceTransportPolicy } from '../src/ice-transport-policy.js'

describe('parseIceTransportPolicy', () => {
  it('accepts all and relay', () => {
    expect(parseIceTransportPolicy('all')).toBe('all')
    expect(parseIceTransportPolicy('RELAY')).toBe('relay')
    expect(parseIceTransportPolicy(' relay ')).toBe('relay')
  })

  it('rejects empty and unknown', () => {
    expect(parseIceTransportPolicy(undefined)).toBeUndefined()
    expect(parseIceTransportPolicy('')).toBeUndefined()
    expect(parseIceTransportPolicy('none')).toBeUndefined()
  })
})

describe('resolveIceTransportPolicy', () => {
  const previous = process.env.WEBRTC_ICE_TRANSPORT_POLICY

  afterEach(() => {
    if (previous === undefined) {
      delete process.env.WEBRTC_ICE_TRANSPORT_POLICY
    } else {
      process.env.WEBRTC_ICE_TRANSPORT_POLICY = previous
    }
  })

  it('defaults to all', () => {
    delete process.env.WEBRTC_ICE_TRANSPORT_POLICY
    expect(resolveIceTransportPolicy()).toBe('all')
  })

  it('reads WEBRTC_ICE_TRANSPORT_POLICY', () => {
    process.env.WEBRTC_ICE_TRANSPORT_POLICY = 'relay'
    expect(resolveIceTransportPolicy()).toBe('relay')
  })

  it('prefers an explicit option over env', () => {
    process.env.WEBRTC_ICE_TRANSPORT_POLICY = 'relay'
    expect(resolveIceTransportPolicy('all')).toBe('all')
  })
})
