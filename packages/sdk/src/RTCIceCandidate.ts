import type { RTCIceCandidateInit } from './types'

/** ICE candidate for trickle ICE exchange over signaling. */
export class RTCIceCandidate {
  readonly candidate: string
  readonly sdpMid: string | null
  readonly sdpMLineIndex: number | null
  readonly usernameFragment: string | null

  /**
   * @param init - Candidate string and SDP metadata from {@link RTCPeerConnection.onicecandidate}.
   */
  constructor(init: RTCIceCandidateInit) {
    this.candidate = init.candidate ?? ''
    this.sdpMid = init.sdpMid ?? null
    this.sdpMLineIndex = init.sdpMLineIndex ?? null
    this.usernameFragment = init.usernameFragment ?? null
  }

  /** Serializes to a plain object for JSON signaling transport. */
  toJSON(): RTCIceCandidateInit {
    return {
      candidate: this.candidate,
      sdpMid: this.sdpMid,
      sdpMLineIndex: this.sdpMLineIndex,
      usernameFragment: this.usernameFragment,
    }
  }
}

export type { RTCIceCandidateInit } from './types'
