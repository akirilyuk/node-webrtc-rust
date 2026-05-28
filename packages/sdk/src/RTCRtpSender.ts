import type { JsRtpSender } from '@node-webrtc-rust/bindings'

import { debugFn } from './debug'
import type { LocalAudioTrack } from './LocalAudioTrack'
import type { MediaStreamTrack } from './MediaStreamTrack'

/**
 * Handle for a local RTP media sender on a peer connection.
 *
 * Returned by {@link RTCPeerConnection.addTrack}. Use {@link replaceTrack} to
 * swap the outbound source without renegotiation when codecs match.
 */
export class RTCRtpSender {
  /** Native sender handle (used by {@link RTCPeerConnection.removeTrack}). */
  readonly native: JsRtpSender
  private _track: MediaStreamTrack | null

  constructor(native: JsRtpSender, track: LocalAudioTrack | null) {
    this.native = native
    this._track = track
  }

  /** Wraps a native sender from {@link RTCPeerConnection.getSenders} or transceivers. */
  static fromNative(native: JsRtpSender, track: LocalAudioTrack | null = null): RTCRtpSender {
    return new RTCRtpSender(native, track)
  }

  /** Stable sender id for this media line. */
  get id(): string {
    return this.native.id
  }

  /** The local track currently being sent, or `null` after {@link replaceTrack}(null). */
  get track(): MediaStreamTrack | null {
    return this._track
  }

  /**
   * Swaps the outbound audio track.
   *
   * @param track - New {@link LocalAudioTrack}, or `null` to detach sending.
   */
  async replaceTrack(track: LocalAudioTrack | null): Promise<void> {
    debugFn('sdk::RTCRtpSender', 'replaceTrack', track ? `id=${track.id}` : 'null')
    await this.native.replaceTrack(track?.native ?? undefined)
    this._track = track
  }
}
