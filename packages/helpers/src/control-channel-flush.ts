import type { RTCDataChannel } from '@node-webrtc-rust/sdk'

/** Poll interval while waiting for SCTP to drain voice-control sends. */
export const VOICE_CONTROL_FLUSH_POLL_MS = 10

/** Max wait before teardown when flushing session_error to the browser. */
export const VOICE_CONTROL_FLUSH_TIMEOUT_MS = 2_000

/**
 * Wait until voice-control {@link RTCDataChannel.bufferedAmount} reaches zero
 * or `timeoutMs` elapses. Returns true when drained, false on timeout or closed channel.
 */
export async function flushVoiceControlChannel(
  controlChannel: RTCDataChannel,
  timeoutMs: number = VOICE_CONTROL_FLUSH_TIMEOUT_MS,
): Promise<boolean> {
  if (controlChannel.readyState !== 'open') return false
  const deadline = Date.now() + timeoutMs
  while (controlChannel.bufferedAmount > 0 && Date.now() < deadline) {
    await new Promise<void>((resolve) => setTimeout(resolve, VOICE_CONTROL_FLUSH_POLL_MS))
    if (controlChannel.readyState !== 'open') return false
  }
  return controlChannel.bufferedAmount === 0
}
