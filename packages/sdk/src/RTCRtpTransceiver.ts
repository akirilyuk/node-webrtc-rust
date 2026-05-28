import type { JsRtpTransceiver } from '@node-webrtc-rust/bindings'

import type { LocalAudioTrack } from './LocalAudioTrack'
import { debugFn } from './debug'
import { RTCRtpReceiver } from './RTCRtpReceiver'
import { RTCRtpSender } from './RTCRtpSender'
import type { RTCRtpTransceiverDirection, TrackKind } from './types'

/**
 * Unified Plan transceiver pairing an {@link RTCRtpSender} and {@link RTCRtpReceiver}.
 */
export class RTCRtpTransceiver {
  readonly native: JsRtpTransceiver
  readonly sender: RTCRtpSender
  readonly receiver: RTCRtpReceiver

  constructor(native: JsRtpTransceiver, senderTrack: LocalAudioTrack | null = null) {
    this.native = native
    this.sender = RTCRtpSender.fromNative(native.sender, senderTrack)
    this.receiver = new RTCRtpReceiver(native.receiver)
  }

  /** SDP mid when negotiated; `null` before offer/answer. */
  get mid(): string | null {
    return this.native.mid ?? null
  }

  /** Desired direction (`sendrecv`, `sendonly`, `recvonly`, `inactive`). */
  get direction(): RTCRtpTransceiverDirection {
    return this.native.direction as RTCRtpTransceiverDirection
  }

  /** Negotiated direction, or `null` if not yet negotiated. */
  get currentDirection(): RTCRtpTransceiverDirection | null {
    const value = this.native.currentDirection
    return value ? (value as RTCRtpTransceiverDirection) : null
  }

  /** `audio` or `video`. */
  get kind(): TrackKind {
    return this.native.kind === 'video' ? 'video' : 'audio'
  }

  /** Whether {@link stop} was called. */
  get stopped(): boolean {
    return this.native.stopped
  }

  /** Updates desired direction; may emit `negotiationneeded`. */
  async setDirection(direction: RTCRtpTransceiverDirection): Promise<void> {
    debugFn('sdk::RTCRtpTransceiver', 'setDirection', direction)
    await this.native.setDirection(direction)
  }

  /** Permanently stops RTP on this transceiver. */
  async stop(): Promise<void> {
    debugFn('sdk::RTCRtpTransceiver', 'stop', `kind=${this.kind}`)
    await this.native.stop()
  }
}
