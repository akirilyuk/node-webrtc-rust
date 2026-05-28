import type { JsRtpReceiver } from '@node-webrtc-rust/bindings'

import { debugFn } from './debug'
import type { TrackKind } from './types'

/**
 * Handle for the receive leg of an {@link RTCRtpTransceiver}.
 *
 * Remote {@link MediaStreamTrack} instances are delivered via {@link RTCPeerConnection.ontrack};
 * {@link track} is reserved for future wiring once a remote track is bound.
 */
export class RTCRtpReceiver {
  readonly native: JsRtpReceiver

  constructor(native: JsRtpReceiver) {
    this.native = native
  }

  /** Stable receiver id for this media line. */
  get id(): string {
    return this.native.id
  }

  /** `audio` or `video`. */
  get kind(): TrackKind {
    return this.native.kind === 'video' ? 'video' : 'audio'
  }

  /**
   * Remote track for this receiver, when available.
   * Populated via {@link RTCPeerConnection.ontrack} in a future release; currently `null`.
   */
  get track(): null {
    return null
  }
}
