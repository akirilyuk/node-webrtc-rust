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

  /** Native handle (used by the native inbound loop). */
  get native(): JsMediaStreamTrack {
    return this.remoteNative
  }

  constructor(native: JsMediaStreamTrack) {
    super(native)
    this.remoteNative = native
  }

  /**
   * Optional listener for every PCM chunk read from this track, in JS `readSample` and in the
   * native inbound loop. Callback is `(data, durationMs)`. Pass `null` to clear.
   */
  setReadSampleTee(callback: ((data: Buffer, durationMs: number) => void) | null): void {
    this.remoteNative.setReadSampleTee(callback ?? undefined)
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
