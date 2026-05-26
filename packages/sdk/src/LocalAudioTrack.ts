import { JsLocalAudioTrack as NativeLocalAudioTrack } from '@node-webrtc-rust/bindings'

import { debugFn } from './debug'
import { MediaStreamTrack } from './MediaStreamTrack'

/**
 * Local audio track backed by the native Opus sample source.
 *
 * Add to a peer connection with {@link RTCPeerConnection.addTrack}, then call
 * {@link writeSample} after the connection is established to deliver RTP to the remote peer.
 */
export class LocalAudioTrack extends MediaStreamTrack {
  readonly native: NativeLocalAudioTrack

  /**
   * @param id - Track id exposed to the remote peer.
   * @param streamId - Stream id grouped in {@link RTCPeerConnection.ontrack} events.
   */
  constructor(id: string, streamId: string) {
    debugFn('sdk::LocalAudioTrack', 'constructor', `id=${id}, streamId=${streamId}`)
    const native = new NativeLocalAudioTrack(id, streamId)
    super(native)
    this.native = native
  }

  /**
   * Writes a PCM audio frame to the track and encodes it for RTP transmission.
   *
   * The remote peer's {@link RTCPeerConnection.ontrack} handler fires when the
   * first frame is sent — negotiation and {@link RTCPeerConnection.addTrack}
   * alone do not surface a remote track on the receiver.
   *
   * @param data - Interleaved stereo 16-bit PCM (3840 bytes for 20 ms at 48 kHz).
   * @param durationMs - Sample duration in milliseconds; defaults to 20.
   */
  async writeSample(data: Buffer | Uint8Array, durationMs = 20): Promise<void> {
    debugFn('sdk::LocalAudioTrack', 'writeSample', `bytes=${data.byteLength}, durationMs=${durationMs}`)
    const buffer = Buffer.isBuffer(data) ? data : Buffer.from(data)
    await this.native.writeSample(buffer, durationMs)
  }
}
