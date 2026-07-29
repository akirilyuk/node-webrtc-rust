import { describe, expect, it, vi } from 'vitest'

import { RTCSessionDescription } from '../src/RTCSessionDescription'
import {
  ensureLocalDescriptionHasIceCredentials,
  LOCAL_DESCRIPTION_ICE_MAX_RETRIES,
} from '../src/local-description-ice'

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

const incompleteOfferSdp = [
  'v=0',
  'o=- 0 0 IN IP4 127.0.0.1',
  's=-',
  't=0 0',
  'm=application 9 UDP/DTLS/SCTP webrtc-datachannel',
  'c=IN IP4 0.0.0.0',
].join('\r\n')

function createDeps(
  initialSdp: string,
  options?: { fixedAfterAttempts?: number },
): {
  deps: Parameters<typeof ensureLocalDescriptionHasIceCredentials>[0]
  calls: {
    createOffer: ReturnType<typeof vi.fn>
    gather: ReturnType<typeof vi.fn>
  }
  setLocal: (sdp: string) => void
} {
  let localSdp = initialSdp
  let offerAttempts = 0
  const createOffer = vi.fn(async () => {
    offerAttempts += 1
    if (options?.fixedAfterAttempts !== undefined && offerAttempts > options.fixedAfterAttempts) {
      localSdp = validOfferSdp
    }
    return new RTCSessionDescription({ type: 'offer', sdp: localSdp })
  })
  const gather = vi.fn(async () => undefined)
  const setLocal = (sdp: string) => {
    localSdp = sdp
  }

  return {
    deps: {
      createOffer,
      createAnswer: vi.fn(
        async () => new RTCSessionDescription({ type: 'answer', sdp: validOfferSdp }),
      ),
      setLocalDescription: vi.fn(async (desc) => {
        localSdp = desc.sdp
      }),
      gatherAndRefreshLocalDescription: gather,
      restartIce: vi.fn(async () => undefined),
      getLocalDescription: () => new RTCSessionDescription({ type: 'offer', sdp: localSdp }),
    },
    calls: { createOffer, gather },
    setLocal,
  }
}

describe('ensureLocalDescriptionHasIceCredentials', () => {
  it('returns when local offer SDP already has ICE credentials', async () => {
    const { deps, calls } = createDeps(validOfferSdp)
    await ensureLocalDescriptionHasIceCredentials(deps)
    expect(calls.createOffer).not.toHaveBeenCalled()
    expect(calls.gather).not.toHaveBeenCalled()
  })

  it('retries with iceRestart offer then succeeds', async () => {
    const { deps, calls } = createDeps(incompleteOfferSdp, { fixedAfterAttempts: 0 })
    const onRetry = vi.fn()
    await ensureLocalDescriptionHasIceCredentials(deps, { onRetry })
    expect(calls.createOffer).toHaveBeenCalledTimes(1)
    expect(calls.createOffer).toHaveBeenCalledWith({ iceRestart: true })
    expect(calls.gather).toHaveBeenCalledTimes(1)
    expect(onRetry).toHaveBeenCalledWith(1, {
      kind: 'offer',
      sdpLen: incompleteOfferSdp.length,
      hasIceUfrag: false,
      hasIcePwd: false,
    })
  })

  it('throws with sdp_len after exhausting retries', async () => {
    const { deps } = createDeps(incompleteOfferSdp)
    await expect(ensureLocalDescriptionHasIceCredentials(deps)).rejects.toThrow(
      `offer SDP missing a=ice-ufrag (sdp_len=${incompleteOfferSdp.length})`,
    )
    expect(deps.createOffer).toHaveBeenCalledTimes(LOCAL_DESCRIPTION_ICE_MAX_RETRIES)
  })
})
