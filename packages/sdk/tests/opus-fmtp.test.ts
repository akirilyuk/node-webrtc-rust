import { describe, expect, test } from 'vitest'

import { opusStereoPreference } from '../src'

const sdpWith = (fmtp: string | null) =>
  ['v=0', 'm=audio 9 UDP/TLS/RTP/SAVPF 111', 'a=rtpmap:111 opus/48000/2']
    .concat(fmtp === null ? [] : [`a=fmtp:111 ${fmtp}`])
    .join('\r\n') + '\r\n'

describe('opusStereoPreference', () => {
  test('Chrome default answer asks for mono', () => {
    expect(opusStereoPreference(sdpWith('minptime=10;useinbandfec=1'))).toBe(false)
  })
  test('stereo=1 asks for stereo', () => {
    expect(opusStereoPreference(sdpWith('minptime=10;useinbandfec=1;stereo=1'))).toBe(true)
  })
  test('stereo=0 is mono', () => {
    expect(opusStereoPreference(sdpWith('minptime=10;stereo=0'))).toBe(false)
  })
  test('sprop-stereo=1 alone does not count', () => {
    expect(opusStereoPreference(sdpWith('minptime=10;sprop-stereo=1'))).toBe(false)
  })
  test('Opus without fmtp is mono', () => {
    expect(opusStereoPreference(sdpWith(null))).toBe(false)
  })
  test('no Opus is unknown', () => {
    expect(
      opusStereoPreference('v=0\r\nm=audio 9 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n'),
    ).toBeNull()
  })
  test('undefined, null and empty are unknown', () => {
    expect(opusStereoPreference(undefined)).toBeNull()
    expect(opusStereoPreference(null)).toBeNull()
    expect(opusStereoPreference('')).toBeNull()
  })
  test('CRLF SDP with stereo=1 and a non-Opus fmtp', () => {
    const sdp = `${sdpWith('stereo=1')}a=fmtp:0 stereo=0\r\n`
    expect(opusStereoPreference(sdp)).toBe(true)
  })
})
