import type { RTCSdpType, RTCSessionDescriptionInit } from './types'

/** Immutable SDP session description (offer, answer, or provisional answer). */
export class RTCSessionDescription {
  readonly type: RTCSdpType
  readonly sdp: string

  /**
   * @param init - SDP type and body from {@link RTCPeerConnection.createOffer} or signaling.
   */
  constructor(init: RTCSessionDescriptionInit) {
    this.type = init.type
    this.sdp = init.sdp
  }

  /** Serializes to a plain object for JSON signaling transport. */
  toJSON(): RTCSessionDescriptionInit {
    return { type: this.type, sdp: this.sdp }
  }
}

export type { RTCSdpType, RTCSessionDescriptionInit } from './types'
