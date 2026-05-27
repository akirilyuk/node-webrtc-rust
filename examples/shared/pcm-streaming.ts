/**
 * PCM streaming conventions for node-webrtc-rust examples.
 *
 * `LocalAudioTrack.writeSample` accepts interleaved stereo PCM. The core
 * engine encodes to the audio codec negotiated during SDP (Opus for WebRTC).
 *
 * RTP timestamp advance comes from `durationMs`, not byte length — mismatched
 * duration produces one blip then silence in browsers.
 *
 * ## Browser receiver (`browser-cosine-chat`)
 *
 * Two-phase stream after `connectionState === 'connected'`:
 *
 * 1. **Prime** — `writeSample(960 B, 5 ms)` silent kick → browser `ontrack`
 * 2. **Stream** — `await writeSample(3840 B, 20 ms)` every 20 ms (one loop per track)
 *
 * ## Node SDK receiver (`audio-cosine`, e2e tests)
 *
 * Same prime pattern, then continuous 3840 B / 20 ms frames.
 */

export const PCM_SAMPLE_RATE = 48_000
export const PCM_CHANNELS = 2
export const PCM_FRAME_DURATION_MS = 20
export const PCM_FULL_FRAME_BYTES =
  PCM_SAMPLE_RATE * (PCM_FRAME_DURATION_MS / 1000) * PCM_CHANNELS * 2

/** Prime/kick frame — 960 bytes = 5 ms stereo PCM at 48 kHz. */
export const PCM_KICK_FRAME_BYTES = 960

/** Must match {@link PCM_KICK_FRAME_BYTES} — pass to writeSample as durationMs. */
export const PCM_KICK_DURATION_MS = 5

export function createKickFrame(): Buffer {
  return Buffer.alloc(PCM_KICK_FRAME_BYTES)
}
