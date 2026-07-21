/**
 * Guards outbound SDP so we never signal an offer/answer that webrtc-rs will
 * reject with `set_remote_description called with no ice-ufrag`.
 *
 * Seen under scale-up burst: client received an offer from the runner that
 * lacked ICE credentials; signaling WS itself was fine (join + peer-joined OK).
 */

export type SdpIceKind = 'offer' | 'answer'

export type SdpIceMeta = {
  kind: SdpIceKind
  sdpLen: number
  hasIceUfrag: boolean
  hasIcePwd: boolean
}

/** Summarize SDP ICE credential presence without logging secrets. */
export function describeSdpIce(sdp: string | undefined | null, kind: SdpIceKind): SdpIceMeta {
  const body = typeof sdp === 'string' ? sdp : ''
  return {
    kind,
    sdpLen: body.length,
    hasIceUfrag: /a=ice-ufrag:/i.test(body),
    hasIcePwd: /a=ice-pwd:/i.test(body),
  }
}

/**
 * Throws when `sdp` is empty or missing `a=ice-ufrag` / `a=ice-pwd`.
 * Call before sending offer/answer over signaling.
 */
export function assertSdpHasIceCredentials(
  sdp: string | undefined | null,
  kind: SdpIceKind,
): SdpIceMeta {
  const meta = describeSdpIce(sdp, kind)
  if (meta.sdpLen === 0) {
    throw new Error(`${kind} SDP missing or empty (sdp_len=0)`)
  }
  if (!meta.hasIceUfrag) {
    throw new Error(`${kind} SDP missing a=ice-ufrag (sdp_len=${meta.sdpLen})`)
  }
  if (!meta.hasIcePwd) {
    throw new Error(`${kind} SDP missing a=ice-pwd (sdp_len=${meta.sdpLen})`)
  }
  return meta
}
