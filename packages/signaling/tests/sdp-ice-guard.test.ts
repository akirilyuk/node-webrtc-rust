import { describe, expect, it } from 'vitest'

import { assertSdpHasIceCredentials, describeSdpIce, SignalingClient } from '../src'

const validOfferSdp = [
  'v=0',
  'o=- 0 0 IN IP4 127.0.0.1',
  's=-',
  't=0 0',
  'a=ice-ufrag:AbCd',
  'a=ice-pwd:abcdefghijklmnopqrstuvwx',
  'm=application 9 UDP/DTLS/SCTP webrtc-datachannel',
  'c=IN IP4 0.0.0.0',
].join('\r\n')

describe('assertSdpHasIceCredentials', () => {
  it('accepts SDP with ice-ufrag and ice-pwd', () => {
    const meta = assertSdpHasIceCredentials(validOfferSdp, 'offer')
    expect(meta.hasIceUfrag).toBe(true)
    expect(meta.hasIcePwd).toBe(true)
    expect(meta.sdpLen).toBe(validOfferSdp.length)
  })

  it('rejects empty SDP', () => {
    expect(() => assertSdpHasIceCredentials('', 'offer')).toThrow(/missing or empty/)
    expect(() => assertSdpHasIceCredentials(undefined, 'answer')).toThrow(/missing or empty/)
  })

  it('rejects SDP without ice-ufrag', () => {
    const sdp = validOfferSdp.replace(/a=ice-ufrag:[^\r\n]+/, '')
    expect(() => assertSdpHasIceCredentials(sdp, 'offer')).toThrow(/missing a=ice-ufrag/)
  })

  it('rejects SDP without ice-pwd', () => {
    const sdp = validOfferSdp.replace(/a=ice-pwd:[^\r\n]+/, '')
    expect(() => assertSdpHasIceCredentials(sdp, 'answer')).toThrow(/missing a=ice-pwd/)
  })

  it('describeSdpIce reports flags without throwing', () => {
    expect(describeSdpIce('v=0\r\n', 'offer')).toEqual({
      kind: 'offer',
      sdpLen: 5,
      hasIceUfrag: false,
      hasIcePwd: false,
    })
  })
})

describe('SignalingClient.sendOffer ice guard', () => {
  it('refuses to send an offer without ice-ufrag', () => {
    const client = new SignalingClient({
      url: 'ws://127.0.0.1:9',
      room: 'r',
      peerId: 'p',
    })
    expect(() =>
      client.sendOffer('peer-b', {
        type: 'offer',
        sdp: 'v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n',
      }),
    ).toThrow(/missing a=ice-ufrag/)
  })
})
