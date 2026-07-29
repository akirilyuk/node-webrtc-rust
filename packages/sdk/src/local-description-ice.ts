import {
  assertSdpHasIceCredentials,
  describeSdpIce,
  type SdpIceKind,
  type SdpIceMeta,
} from '@node-webrtc-rust/signaling'

import type { RTCSessionDescription } from './RTCSessionDescription'
import type { RTCOfferOptions } from './types'

/** Max ICE-restart retries after gather when local SDP lacks credentials. */
export const LOCAL_DESCRIPTION_ICE_MAX_RETRIES = 2

export type LocalDescriptionIceRetryDeps = {
  createOffer: (options?: RTCOfferOptions) => Promise<RTCSessionDescription>
  createAnswer: () => Promise<RTCSessionDescription>
  setLocalDescription: (desc: RTCSessionDescription) => Promise<void>
  /** Native gather + refresh localDescription only (no ICE guard recursion). */
  gatherAndRefreshLocalDescription: () => Promise<void>
  restartIce: () => Promise<void>
  getLocalDescription: () => RTCSessionDescription | null
}

export type LocalDescriptionIceRetryOptions = {
  maxRetries?: number
  onRetry?: (attempt: number, meta: SdpIceMeta) => void
}

/**
 * Ensures {@link getLocalDescription} includes `a=ice-ufrag` and `a=ice-pwd`.
 * Retries with ICE restart when credentials are missing after gathering.
 */
export async function ensureLocalDescriptionHasIceCredentials(
  deps: LocalDescriptionIceRetryDeps,
  options?: LocalDescriptionIceRetryOptions,
): Promise<void> {
  const maxRetries = options?.maxRetries ?? LOCAL_DESCRIPTION_ICE_MAX_RETRIES

  for (let attempt = 0; attempt <= maxRetries; attempt++) {
    const local = deps.getLocalDescription()
    const kind = local?.type
    if (kind !== 'offer' && kind !== 'answer') {
      throw new Error('local description missing or invalid type before ICE validation (sdp_len=0)')
    }

    const meta = describeSdpIce(local?.sdp, kind as SdpIceKind)
    if (meta.hasIceUfrag && meta.hasIcePwd) {
      return
    }

    if (attempt >= maxRetries) {
      assertSdpHasIceCredentials(local?.sdp, kind as SdpIceKind)
      return
    }

    options?.onRetry?.(attempt + 1, meta)

    if (kind === 'offer') {
      const offer = await deps.createOffer({ iceRestart: true })
      await deps.setLocalDescription(offer)
    } else {
      await deps.restartIce()
      const answer = await deps.createAnswer()
      await deps.setLocalDescription(answer)
    }
    await deps.gatherAndRefreshLocalDescription()
  }
}
