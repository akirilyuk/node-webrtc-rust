import { JsLocalAudioTrack as NativeLocalAudioTrack } from '@node-webrtc-rust/bindings'

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
    const native = new NativeLocalAudioTrack(id, streamId)
    super(native)
    this.native = native
  }

  /**
   * Writes a PCM audio frame to the track.
   * @param data - Raw PCM samples (typically 960 bytes for 20 ms at 48 kHz mono).
   * @param durationMs - Sample duration in milliseconds; defaults to 20.
   */
  async writeSample(data: Buffer | Uint8Array, durationMs = 20): Promise<void> {
    const buffer = Buffer.isBuffer(data) ? data : Buffer.from(data)
    await this.native.writeSample(buffer, durationMs)
  }
}
