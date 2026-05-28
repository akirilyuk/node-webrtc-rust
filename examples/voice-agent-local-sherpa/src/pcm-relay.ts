/**
 * PCM helpers for voice examples (silence tail, optional relay).
 *
 * Roundtrip uses two VoiceAgents over WebRTC (speaker → listener); it does not use relay.
 */

import type { LocalAudioTrack, RemoteAudioTrack } from '@node-webrtc-rust/sdk'

import {
  PCM_FRAME_DURATION_MS,
  PCM_FULL_FRAME_BYTES,
  PCM_KICK_DURATION_MS,
  PCM_KICK_FRAME_BYTES,
} from '../../shared/pcm-streaming.js'

const PCM_SAMPLE_RATE = 48_000
const PCM_CHANNELS = 2

/** Infer writeSample duration from interleaved stereo s16le @ 48 kHz. */
export function stereoPcmDurationMs(byteLength: number): number {
  if (byteLength <= 0) return 1
  if (byteLength === PCM_KICK_FRAME_BYTES) return PCM_KICK_DURATION_MS
  const samplesPerChannel = byteLength / (PCM_CHANNELS * 2)
  return Math.max(1, Math.round((samplesPerChannel * 1000) / PCM_SAMPLE_RATE))
}

/**
 * Copies frames from `source` (remote receive) to `sink` (local send) until `shouldRun()` is false.
 */
export async function relayRemoteAudioToLocal(
  source: RemoteAudioTrack,
  sink: LocalAudioTrack,
  shouldRun: () => boolean,
): Promise<void> {
  while (shouldRun()) {
    try {
      const pcm = await source.readSample()
      if (pcm.length === 0) {
        await sleep(10)
        continue
      }
      await sink.writeSample(pcm, stereoPcmDurationMs(pcm.length))
    } catch {
      await sleep(20)
    }
  }
}

/**
 * Sends silent 20 ms frames at real-time pace (one frame every {@link PCM_FRAME_DURATION_MS} ms).
 * Matches browser cosine / live capture timing so VAD hold and silence endpoint behave like production.
 */
export async function streamSilence(sink: LocalAudioTrack, seconds: number): Promise<void> {
  if (seconds <= 0) return
  const frameCount = Math.ceil((seconds * 1000) / PCM_FRAME_DURATION_MS)
  const silent = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  const endAt = performance.now() + seconds * 1000
  for (let i = 0; i < frameCount; i++) {
    await sink.writeSample(silent, PCM_FRAME_DURATION_MS)
    const nextTickAt = endAt - (frameCount - i - 1) * PCM_FRAME_DURATION_MS
    const waitMs = nextTickAt - performance.now()
    if (waitMs > 0) await sleep(waitMs)
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}
