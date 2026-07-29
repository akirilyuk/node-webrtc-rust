import { describe, expect, it, vi } from 'vitest'

import { RTCSessionDescription } from '@node-webrtc-rust/sdk'

import { createOfferGatherWithIceCredentials } from '../src/offer-ice-gather.js'

function createMockPeerConnection(options: { failGatherAttempts?: number; sdpAfterFix?: string }) {
  const incompleteSdp = 'v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n'
  const validSdp =
    options.sdpAfterFix ??
    [
      'v=0',
      'a=ice-ufrag:AbCd',
      'a=ice-pwd:abcdefghijklmnopqrstuvwx',
      'm=application 9 UDP/DTLS/SCTP webrtc-datachannel',
    ].join('\r\n')

  let gatherFailures = 0
  let localSdp = incompleteSdp

  const pc = {
    createOffer: vi.fn(async (opts?: { iceRestart?: boolean }) => {
      if (opts?.iceRestart) {
        localSdp = validSdp
      }
      return new RTCSessionDescription({ type: 'offer', sdp: localSdp })
    }),
    setLocalDescription: vi.fn(async (desc: RTCSessionDescription) => {
      localSdp = desc.sdp
    }),
    gatheringComplete: vi.fn(async () => {
      gatherFailures += 1
      if (
        options.failGatherAttempts !== undefined &&
        gatherFailures <= options.failGatherAttempts
      ) {
        throw new Error(`offer SDP missing a=ice-ufrag (sdp_len=${localSdp.length})`)
      }
      if (!/a=ice-ufrag:/i.test(localSdp)) {
        throw new Error(`offer SDP missing a=ice-ufrag (sdp_len=${localSdp.length})`)
      }
    }),
    localDescription: null as RTCSessionDescription | null,
  }

  Object.defineProperty(pc, 'localDescription', {
    get() {
      return new RTCSessionDescription({ type: 'offer', sdp: localSdp })
    },
  })

  return pc
}

describe('createOfferGatherWithIceCredentials', () => {
  it('completes on first successful gather', async () => {
    const pc = createMockPeerConnection({ failGatherAttempts: 0 })
    pc.gatheringComplete = vi.fn(async () => undefined)
    pc.createOffer = vi.fn(
      async () =>
        new RTCSessionDescription({ type: 'offer', sdp: 'a=ice-ufrag:x\r\na=ice-pwd:y\r\n' }),
    )

    await createOfferGatherWithIceCredentials(pc as never)
    expect(pc.createOffer).toHaveBeenCalledTimes(1)
    expect(pc.createOffer).toHaveBeenCalledWith(undefined)
    expect(pc.gatheringComplete).toHaveBeenCalledTimes(1)
  })

  it('retries with iceRestart when gather throws missing ice-ufrag', async () => {
    const pc = createMockPeerConnection({ failGatherAttempts: 1 })
    const onRetry = vi.fn()

    await createOfferGatherWithIceCredentials(pc as never, { onRetry })

    expect(pc.createOffer).toHaveBeenCalledTimes(2)
    expect(pc.createOffer).toHaveBeenNthCalledWith(1, undefined)
    expect(pc.createOffer).toHaveBeenNthCalledWith(2, { iceRestart: true })
    expect(pc.gatheringComplete).toHaveBeenCalledTimes(2)
    expect(onRetry).toHaveBeenCalledWith(1, expect.any(Number))
  })

  it('rethrows after max helper retries', async () => {
    const pc = createMockPeerConnection({ failGatherAttempts: 99 })

    await expect(createOfferGatherWithIceCredentials(pc as never)).rejects.toThrow(
      /missing a=ice-ufrag/,
    )
    expect(pc.createOffer).toHaveBeenCalledTimes(3)
  })
})
