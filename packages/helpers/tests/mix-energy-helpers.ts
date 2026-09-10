import type { RemoteAudioTrack } from '@node-webrtc-rust/sdk'

import type { ClientAudioMixer } from '../src/client-audio-mixer.js'
import { PCM_FRAME_DURATION_MS, PCM_FULL_FRAME_BYTES } from '../src/pcm.js'
import { delay } from './mix-three-client-helpers.js'

type MixerTtsTestAccess = ClientAudioMixer & {
  peers: Map<string, { pendingTts: Buffer | null }>
}

const DEFAULT_LOUDER_RATIO = 1.5
/** Default peak-RMS ratio: muted mix must stay below baseline * this factor. */
const DEFAULT_QUIETER_RATIO = 0.35

export const LOUD_AMPLITUDE = 12_000

export function createLoudStereoFrame(amplitude = LOUD_AMPLITUDE): Buffer {
  const frames = PCM_FULL_FRAME_BYTES / 4
  const out = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  for (let i = 0; i < frames; i++) {
    out.writeInt16LE(amplitude, i * 4)
    out.writeInt16LE(amplitude, i * 4 + 2)
  }
  return out
}

/** Left-channel-only loud PCM (mirrors Piper/Sherpa mono TTS before downmix pan). */
export function createLoudLeftOnlyFrame(amplitude = LOUD_AMPLITUDE): Buffer {
  const frames = PCM_FULL_FRAME_BYTES / 4
  const out = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  for (let i = 0; i < frames; i++) {
    out.writeInt16LE(amplitude, i * 4)
    out.writeInt16LE(0, i * 4 + 2)
  }
  return out
}

export function stereoRmsFromSplitChannels(
  left: Int16Array,
  right: Int16Array,
): { left: number; right: number } {
  const pairs = Math.min(left.length, right.length)
  if (pairs === 0) {
    return { left: 0, right: 0 }
  }

  let sumL = 0
  let sumR = 0
  for (let i = 0; i < pairs; i++) {
    sumL += left[i] * left[i]
    sumR += right[i] * right[i]
  }

  return {
    left: Math.sqrt(sumL / pairs),
    right: Math.sqrt(sumR / pairs),
  }
}

export function stereoRms(pcm: Uint8Array | Buffer): { left: number; right: number } {
  const bytes = Buffer.isBuffer(pcm) ? pcm : Buffer.from(pcm)
  const pairs = Math.floor(bytes.byteLength / 4)
  if (pairs === 0) {
    return { left: 0, right: 0 }
  }

  let sumL = 0
  let sumR = 0
  for (let i = 0; i < pairs; i++) {
    const l = bytes.readInt16LE(i * 4)
    const r = bytes.readInt16LE(i * 4 + 2)
    sumL += l * l
    sumR += r * r
  }

  return {
    left: Math.sqrt(sumL / pairs),
    right: Math.sqrt(sumR / pairs),
  }
}

export function assertRightLouder(left: number, right: number, ratio = DEFAULT_LOUDER_RATIO): void {
  if (!(right > left * ratio)) {
    throw new Error(
      `expected right RMS > left * ${ratio} (left=${left.toFixed(1)} right=${right.toFixed(1)})`,
    )
  }
}

export function assertLeftLouder(left: number, right: number, ratio = DEFAULT_LOUDER_RATIO): void {
  if (!(left > right * ratio)) {
    throw new Error(
      `expected left RMS > right * ${ratio} (left=${left.toFixed(1)} right=${right.toFixed(1)})`,
    )
  }
}

export function mergeStereoRms(
  a: { left: number; right: number },
  b: { left: number; right: number },
): { left: number; right: number } {
  return {
    left: Math.max(a.left, b.left),
    right: Math.max(a.right, b.right),
  }
}

function peakStereoRms(stereo: { left: number; right: number }): number {
  return Math.max(stereo.left, stereo.right)
}

/**
 * Assert muted inbound mix energy dropped vs an unmuted baseline probe.
 * Compares max(left, right) peak RMS: muted must be < baseline peak * ratio.
 */
export function assertMuchQuieter(
  muted: { left: number; right: number },
  baseline: { left: number; right: number },
  ratio = DEFAULT_QUIETER_RATIO,
): void {
  const mutedPeak = peakStereoRms(muted)
  const baselinePeak = peakStereoRms(baseline)
  if (!(mutedPeak < baselinePeak * ratio)) {
    throw new Error(
      `expected muted peak RMS < baseline peak * ${ratio} (muted=${mutedPeak.toFixed(1)} baseline=${baselinePeak.toFixed(1)} left=${muted.left.toFixed(1)} right=${muted.right.toFixed(1)})`,
    )
  }
}

export type StereoEnergyReader = {
  readSample(): Promise<Uint8Array | null | undefined>
}

/**
 * Poll inbound stereo PCM until max(L,R) RMS exceeds threshold (TTS/STT lag safe).
 */
export async function waitForInboundStereoEnergy(
  track: StereoEnergyReader,
  options: {
    threshold: number
    timeoutMs: number
    label?: string
  },
): Promise<void> {
  const { threshold, timeoutMs, label = 'inbound stereo energy' } = options
  const endAt = Date.now() + timeoutMs
  while (Date.now() < endAt) {
    const sample = await track.readSample()
    if (sample && sample.byteLength >= 4) {
      const frame = stereoRms(sample)
      const peak = Math.max(frame.left, frame.right)
      if (peak > threshold) {
        return
      }
    }
    await delay(5)
  }
  throw new Error(
    `timed out waiting for ${label}: max(L,R) RMS did not exceed ${threshold} within ${timeoutMs}ms`,
  )
}

/**
 * Poll inbound stereo PCM until max(L,R) RMS stays below threshold for a
 * consecutive quiet window (drain leftover TTS before the next utterance).
 */
export async function waitForInboundStereoQuiet(
  track: StereoEnergyReader,
  options: {
    threshold: number
    quietWindowMs: number
    timeoutMs: number
    label?: string
    pollIntervalMs?: number
  },
): Promise<void> {
  const {
    threshold,
    quietWindowMs,
    timeoutMs,
    label = 'inbound stereo quiet',
    pollIntervalMs = 5,
  } = options
  const endAt = Date.now() + timeoutMs
  let quietSince: number | null = null

  while (Date.now() < endAt) {
    const sample = await track.readSample()
    let peak = 0
    if (sample && sample.byteLength >= 4) {
      const frame = stereoRms(sample)
      peak = Math.max(frame.left, frame.right)
    }

    if (peak < threshold) {
      if (quietSince === null) {
        quietSince = Date.now()
      } else if (Date.now() - quietSince >= quietWindowMs) {
        return
      }
    } else {
      quietSince = null
    }

    await delay(pollIntervalMs)
  }

  throw new Error(
    `timed out waiting for ${label}: max(L,R) RMS did not stay below ${threshold} for ${quietWindowMs}ms within ${timeoutMs}ms`,
  )
}

/** Peak L/R RMS over a duration (matches staging voice-data-mix-smoke accumulate). */
export async function accumulateInboundStereoRms(
  track: RemoteAudioTrack,
  durationMs: number,
): Promise<{ left: number; right: number }> {
  let peak = { left: 0, right: 0 }
  const endAt = Date.now() + durationMs
  while (Date.now() < endAt) {
    const sample = await track.readSample()
    if (sample && sample.byteLength >= 4) {
      const frame = stereoRms(sample)
      peak = mergeStereoRms(peak, frame)
    }
    await delay(5)
  }
  return peak
}

/**
 * Peak L/R RMS using only frames where the expected channel already dominates.
 * Avoids max(L)/max(R) from different pose phases in one window (TTS pose flip).
 */
export async function accumulateDirectionalStereoRms(
  track: RemoteAudioTrack,
  durationMs: number,
  louderSide: 'left' | 'right',
): Promise<{ left: number; right: number }> {
  let peak = { left: 0, right: 0 }
  const endAt = Date.now() + durationMs
  while (Date.now() < endAt) {
    const sample = await track.readSample()
    if (sample && sample.byteLength >= 4) {
      const frame = stereoRms(sample)
      const matches = louderSide === 'left' ? frame.left > frame.right : frame.right > frame.left
      if (matches) {
        peak = mergeStereoRms(peak, frame)
      }
    }
    await delay(5)
  }
  return peak
}

export async function pumpLoudMicFrames(
  writeSample: (data: Buffer, durationMs: number) => Promise<void>,
  durationMs: number,
): Promise<void> {
  const frame = createLoudStereoFrame()
  const endAt = Date.now() + durationMs
  while (Date.now() < endAt) {
    await writeSample(frame, PCM_FRAME_DURATION_MS)
    await delay(PCM_FRAME_DURATION_MS)
  }
}

/**
 * Simulates VoiceAgent native TTS drain into the mix pump. JS {@link LocalAudioTrack.writeSample}
 * does not invoke {@link LocalAudioTrack.setWriteSampleTee}; only native notify_write_tee does.
 */
export function injectTtsSidecarPendingFrame(
  mixer: ClientAudioMixer,
  peerId: string,
  pcm: Buffer,
): void {
  if (pcm.length !== PCM_FULL_FRAME_BYTES) {
    throw new Error(`TTS sidecar frame must be ${PCM_FULL_FRAME_BYTES} bytes, got ${pcm.length}`)
  }
  const state = (mixer as MixerTtsTestAccess).peers.get(peerId)
  if (!state) {
    throw new Error(`ClientAudioMixer peer ${peerId} is not registered`)
  }
  state.pendingTts = Buffer.from(pcm)
}

/** Loud constant PCM into pendingTts each 20 ms tick (mirrors wired sidecar tee semantics). */
export async function pumpLoudTtsSidecarFrames(
  mixer: ClientAudioMixer,
  peerId: string,
  durationMs: number,
): Promise<void> {
  const frame = createLoudStereoFrame()
  const endAt = Date.now() + durationMs
  while (Date.now() < endAt) {
    injectTtsSidecarPendingFrame(mixer, peerId, frame)
    await delay(PCM_FRAME_DURATION_MS)
  }
}

/** Left-only loud PCM into pendingTts each 20 ms tick (catches pre-downmix pan regressions). */
export async function pumpLoudTtsLeftOnlySidecarFrames(
  mixer: ClientAudioMixer,
  peerId: string,
  durationMs: number,
  options?: { signal?: AbortSignal },
): Promise<void> {
  const frame = createLoudLeftOnlyFrame()
  const endAt = Date.now() + durationMs
  const signal = options?.signal
  while (Date.now() < endAt) {
    if (signal?.aborted) {
      return
    }
    injectTtsSidecarPendingFrame(mixer, peerId, frame)
    await delay(PCM_FRAME_DURATION_MS)
    if (signal?.aborted) {
      return
    }
  }
}
