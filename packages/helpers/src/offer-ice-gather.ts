import { describeSdpIce } from '@node-webrtc-rust/signaling'
import { LOCAL_DESCRIPTION_ICE_MAX_RETRIES, RTCPeerConnection } from '@node-webrtc-rust/sdk'

function isMissingIceCredentialError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error)
  return /missing a=ice-ufrag|missing a=ice-pwd|missing or empty/i.test(message)
}

/**
 * Creates an offer, applies it locally, waits for gathering (with SDK ICE guard),
 * and retries the full flow with `iceRestart` when credentials are still missing.
 */
export async function createOfferGatherWithIceCredentials(
  pc: RTCPeerConnection,
  options?: {
    onRetry?: (attempt: number, sdpLen: number) => void
  },
): Promise<void> {
  for (let attempt = 0; attempt <= LOCAL_DESCRIPTION_ICE_MAX_RETRIES; attempt++) {
    try {
      const offer = await pc.createOffer(attempt > 0 ? { iceRestart: true } : undefined)
      await pc.setLocalDescription(offer)
      await pc.gatheringComplete()
      return
    } catch (error) {
      if (attempt >= LOCAL_DESCRIPTION_ICE_MAX_RETRIES || !isMissingIceCredentialError(error)) {
        throw error
      }
      const meta = describeSdpIce(pc.localDescription?.sdp, 'offer')
      options?.onRetry?.(attempt + 1, meta.sdpLen)
    }
  }
}
