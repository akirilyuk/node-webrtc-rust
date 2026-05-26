import type { RTCIceCandidateInit } from './types'

export class RTCIceCandidate implements RTCIceCandidateInit {
  readonly candidate: string
  readonly sdpMid: string | null
  readonly sdpMLineIndex: number | null
  readonly usernameFragment: string | null

  constructor(init: RTCIceCandidateInit = {}) {
    this.candidate = init.candidate ?? ''
    this.sdpMid = init.sdpMid ?? null
    this.sdpMLineIndex = init.sdpMLineIndex ?? null
    this.usernameFragment = init.usernameFragment ?? null
  }

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
