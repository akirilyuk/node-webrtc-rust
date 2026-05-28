/**
 * Re-export PCM streaming conventions from @node-webrtc-rust/helpers.
 *
 * Examples may import from here (relative path) or from the helpers package directly.
 */

export {
  PCM_SAMPLE_RATE,
  PCM_CHANNELS,
  PCM_FRAME_DURATION_MS,
  PCM_FULL_FRAME_BYTES,
  PCM_KICK_FRAME_BYTES,
  PCM_KICK_DURATION_MS,
  createKickFrame,
} from '@node-webrtc-rust/helpers/pcm'
