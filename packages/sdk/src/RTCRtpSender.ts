import type { MediaStreamTrack } from './MediaStreamTrack'

/**
 * Handle for a local RTP media sender on a peer connection.
 *
 * Returned by {@link RTCPeerConnection.addTrack}. v0.1 exposes the attached
 * track only; replaceTrack and transport stats are deferred.
 */
export class RTCRtpSender {
  /** The local track currently being sent. */
  readonly track: MediaStreamTrack

  constructor(track: MediaStreamTrack) {
    this.track = track
  }
}
