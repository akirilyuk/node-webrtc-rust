import type { JsMediaStreamTrack } from '@node-webrtc-rust/bindings'

import { debugFn } from './debug'
import { MediaStreamTrack } from './MediaStreamTrack'

/**
 * Remote audio track with Opus → PCM decode via {@link readSample}.
 *
 * Returned from {@link RTCPeerConnection.ontrack} for inbound audio after the
 * remote peer has sent at least one RTP packet.
 */
export class RemoteAudioTrack extends MediaStreamTrack {
  private readonly remoteNative: JsMediaStreamTrack

  constructor(native: JsMediaStreamTrack) {
    super(native)
    this.remoteNative = native
  }

  /**
   * Reads and decodes the next audio sample (typically 3840 bytes = 20 ms stereo PCM).
   */
  async readSample(): Promise<Buffer> {
    debugFn('sdk::RemoteAudioTrack', 'readSample', `id=${this.id}`)
    const data = await this.remoteNative.readSample()
    return Buffer.isBuffer(data) ? data : Buffer.from(data)
  }
}
