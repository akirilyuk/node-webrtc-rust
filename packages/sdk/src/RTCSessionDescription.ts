import type { RTCSdpType, RTCSessionDescriptionInit } from './types'

export class RTCSessionDescription implements RTCSessionDescriptionInit {
  readonly type: RTCSdpType
  readonly sdp: string

  constructor(init: RTCSessionDescriptionInit) {
    this.type = init.type
    this.sdp = init.sdp
  }

  toJSON(): RTCSessionDescriptionInit {
    return { type: this.type, sdp: this.sdp }
  }
}

export type { RTCSessionDescriptionInit, RTCSdpType } from './types'
