import { describe, expect, it } from 'vitest'

/** True when SDP negotiates an audio m-line (used by data-only mode tests). */
export function sdpHasAudioMedia(sdp: string): boolean {
  return /^m=audio/m.test(sdp)
}

describe('data-only session mode', () => {
  it('sdpHasAudioMedia detects audio m-lines', () => {
    expect(sdpHasAudioMedia('v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n')).toBe(true)
    expect(sdpHasAudioMedia('v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n')).toBe(
      false,
    )
  })
})
