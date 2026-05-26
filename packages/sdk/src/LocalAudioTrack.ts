import { JsLocalAudioTrack as NativeLocalAudioTrack } from '@node-webrtc-rust/bindings'

import { MediaStreamTrack } from './MediaStreamTrack'

export class LocalAudioTrack extends MediaStreamTrack {
  readonly native: NativeLocalAudioTrack

  constructor(id: string, streamId: string) {
    const native = new NativeLocalAudioTrack(id, streamId)
    super(native)
    this.native = native
  }

  async writeSample(data: Buffer | Uint8Array, durationMs = 20): Promise<void> {
    const buffer = Buffer.isBuffer(data) ? data : Buffer.from(data)
    await this.native.writeSample(buffer, durationMs)
  }
}
