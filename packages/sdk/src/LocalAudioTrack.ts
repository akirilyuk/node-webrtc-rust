import { JsLocalAudioTrack as NativeLocalAudioTrack } from '@node-webrtc-rust/bindings'

import { debugFn } from './debug'
import { MediaStreamTrack } from './MediaStreamTrack'

/**
 * Local audio track backed by the native PCM → RTP pipeline.
 *
 * Add to a peer connection with {@link RTCPeerConnection.addTrack}, then call
 * {@link writeSample} with **interleaved stereo PCM** after the connection is
 * established. The native layer encodes PCM to whatever audio codec was
 * negotiated during SDP (Opus for WebRTC peers).
 *
 * ## Streaming lifecycle (read this before integrating audio)
 *
 * Negotiation alone does **not** produce a remote track. Every sender must follow
 * this two-phase pattern after `connectionState === 'connected'`:
 *
 * 1. **Prime** — one `writeSample` with a **960-byte** buffer and **`durationMs: 5`**
 *    (5 ms of PCM). This delivers the first RTP packet and kicks the receiver's `ontrack`.
 * 2. **Stream** — repeated `writeSample` with **3840-byte** frames (20 ms of
 *    48 kHz stereo PCM). Await each call before sending the next.
 *
 * `durationMs` sets the RTP timestamp advance — it must match the PCM byte length
 * (960 B → 5 ms, 3840 B → 20 ms). A mismatched duration causes one blip then silence.
 *
 * @see {@link PCM_KICK_FRAME_BYTES} in `examples/shared/pcm-streaming.ts`
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
   * Writes interleaved stereo PCM. Encoded to the negotiated RTP codec before send.
   *
   * @param data - Interleaved stereo 16-bit PCM (3840 bytes for 20 ms at 48 kHz).
   * @param durationMs - Sample duration in milliseconds; defaults to 20.
   */
  async writeSample(data: Buffer | Uint8Array, durationMs = 20): Promise<void> {
    debugFn(
      'sdk::LocalAudioTrack',
      'writeSample',
      `bytes=${data.byteLength}, durationMs=${durationMs}`,
    )
    const buffer = Buffer.isBuffer(data) ? data : Buffer.from(data)
    await this.native.writeSample(buffer, durationMs)
  }
}
