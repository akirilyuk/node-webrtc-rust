import type { ClientAudioMixer } from '../src/client-audio-mixer.js'
import { PCM_FRAME_DURATION_MS, PCM_FULL_FRAME_BYTES } from '../src/pcm.js'
import { delay } from './mix-three-client-helpers.js'

type MixerTtsTestAccess = ClientAudioMixer & {
  peers: Map<string, { ttsQueue: Buffer[]; lastSidecarEnqueueAt: number | null }>
}

const DEFAULT_LOUDER_RATIO = 1.5
/** Default peak-RMS ratio: muted mix must stay below baseline * this factor. */
const DEFAULT_QUIETER_RATIO = 0.35

export const LOUD_AMPLITUDE = 12_000
/**
 * Probe tone frequency. Must be a multiple of 50 Hz so every 20 ms frame holds whole cycles and
 * identical frames stay phase-continuous. A constant (DC) "loud" frame is NOT a valid stimulus:
 * Opus rejects DC, so after a ~200 ms step transient the decoded PCM is silent even though the
 * sender wrote a loud frame every 20 ms.
 */
export const LOUD_TONE_HZ = 500

function toneSample(index: number, amplitude: number): number {
  return Math.round(amplitude * Math.sin((2 * Math.PI * LOUD_TONE_HZ * index) / 48_000))
}

export function createLoudStereoFrame(amplitude = LOUD_AMPLITUDE): Buffer {
  const frames = PCM_FULL_FRAME_BYTES / 4
  const out = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  for (let i = 0; i < frames; i++) {
    const sample = toneSample(i, amplitude)
    out.writeInt16LE(sample, i * 4)
    out.writeInt16LE(sample, i * 4 + 2)
  }
  return out
}

/** Left-channel-only loud PCM (mirrors Piper/Sherpa mono TTS before downmix pan). */
export function createLoudLeftOnlyFrame(amplitude = LOUD_AMPLITUDE): Buffer {
  const frames = PCM_FULL_FRAME_BYTES / 4
  const out = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  for (let i = 0; i < frames; i++) {
    out.writeInt16LE(toneSample(i, amplitude), i * 4)
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

export type LiveInboundReader = StereoEnergyReader & {
  /** Frames drained so far (diagnostics). */
  readonly framesRead: number
  stop(): void
}

type LiveReaderWaiter = {
  resolve: (sample: Uint8Array | null | undefined) => void
  reject: (error: unknown) => void
}

/**
 * Continuously drains `track.readSample()` in the background (like real playout) and hands each
 * newly arrived frame to whoever is awaiting `readSample()`. No receive backlog can build up
 * between probe windows, so energy probes measure live audio, not stale RTP.
 */
export function createLiveInboundReader(track: StereoEnergyReader): LiveInboundReader {
  let framesRead = 0
  let stopped = false
  let pendingWaiters: LiveReaderWaiter[] = []

  const resolveAllWaiters = (sample: Uint8Array | null | undefined): void => {
    const waiters = pendingWaiters
    pendingWaiters = []
    for (const waiter of waiters) {
      waiter.resolve(sample)
    }
  }

  const rejectAllWaiters = (error: unknown): void => {
    const waiters = pendingWaiters
    pendingWaiters = []
    for (const waiter of waiters) {
      waiter.reject(error)
    }
  }

  async function drainLoop(): Promise<void> {
    for (;;) {
      if (stopped) {
        break
      }
      try {
        const sample = await track.readSample()
        framesRead++
        if (stopped) {
          break
        }
        resolveAllWaiters(sample)
      } catch (error) {
        if (stopped) {
          break
        }
        rejectAllWaiters(error)
        stopped = true
        break
      }
    }
  }

  void drainLoop()

  return {
    get framesRead() {
      return framesRead
    },
    readSample(): Promise<Uint8Array | null | undefined> {
      if (stopped) {
        return Promise.resolve(null)
      }
      return new Promise((resolve, reject) => {
        pendingWaiters.push({ resolve, reject })
      })
    },
    stop(): void {
      if (stopped) {
        return
      }
      stopped = true
      resolveAllWaiters(null)
    },
  }
}

/**
 * Run-length stereo frame classifier for pan-probe failure dumps: `q` quiet, `L` left-only,
 * `R` right-only, `D` dual (both channels), `n` null/short sample. One lane per label
 * (e.g. `tx` sender writes vs `rx` listener reads) so ordering across the WebRTC hop is visible.
 */
export class StereoTimeline {
  private readonly startedAt = Date.now()
  private readonly lanes = new Map<string, Array<{ cls: string; count: number; atMs: number }>>()

  note(lane: string, sample: Uint8Array | null | undefined, quietThreshold = 200): void {
    let cls = 'n'
    if (sample && sample.byteLength >= 4) {
      const { left, right } = stereoRms(sample)
      const l = left > quietThreshold
      const r = right > quietThreshold
      cls = l && r ? 'D' : l ? 'L' : r ? 'R' : 'q'
      if (sample.byteLength !== PCM_FULL_FRAME_BYTES) {
        cls = `${cls}[${sample.byteLength}B]`
      }
    }
    const runs = this.lanes.get(lane) ?? []
    const last = runs[runs.length - 1]
    if (last && last.cls === cls) {
      last.count += 1
    } else {
      runs.push({ cls, count: 1, atMs: Date.now() - this.startedAt })
    }
    this.lanes.set(lane, runs)
  }

  mark(lane: string, label: string): void {
    const runs = this.lanes.get(lane) ?? []
    runs.push({ cls: `|${label}|`, count: 1, atMs: Date.now() - this.startedAt })
    this.lanes.set(lane, runs)
  }

  dump(): string {
    const lines: string[] = []
    for (const [lane, runs] of this.lanes) {
      lines.push(
        `${lane}: ${runs.map((r) => `${r.atMs}ms:${r.cls}${r.count > 1 ? `×${r.count}` : ''}`).join(' ')}`,
      )
    }
    return lines.join('\n')
  }
}

/** Wraps a reader so every read is classified into `timeline` lane `lane`. */
export function traceStereoReads(
  track: StereoEnergyReader,
  timeline: StereoTimeline,
  lane: string,
): StereoEnergyReader {
  return {
    async readSample() {
      const sample = await track.readSample()
      timeline.note(lane, sample)
      return sample
    },
  }
}

type MixerOutboundTestAccess = ClientAudioMixer & {
  peers: Map<
    string,
    { pcOutbound?: { writeSample(pcm: Buffer, durationMs: number): Promise<void> } }
  >
}

/**
 * Wraps the mixer's PC outbound track for `peerId` so every pump/burst write is classified
 * into `timeline` lane `lane`. Takes effect on the next pump resume (pose flip / mute flush).
 */
export function traceMixerOutboundWrites(
  mixer: ClientAudioMixer,
  peerId: string,
  timeline: StereoTimeline,
  lane: string,
): void {
  const state = (mixer as MixerOutboundTestAccess).peers.get(peerId)
  const out = state?.pcOutbound
  if (!state || !out) {
    throw new Error(`ClientAudioMixer peer ${peerId} has no PC outbound to trace`)
  }
  state.pcOutbound = {
    async writeSample(pcm: Buffer, durationMs: number) {
      timeline.note(lane, pcm)
      await out.writeSample(pcm, durationMs)
    },
  }
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

/** Wait until inbound stereo is clearly left-dominant (optional gate; not used by staging smokes). */
export async function waitForInboundStereoLeftDominant(
  track: StereoEnergyReader,
  options: {
    ratio?: number
    timeoutMs: number
    label?: string
  },
): Promise<void> {
  const {
    ratio = DEFAULT_LOUDER_RATIO,
    timeoutMs,
    label = 'inbound left-dominant stereo',
  } = options
  const endAt = Date.now() + timeoutMs
  while (Date.now() < endAt) {
    const sample = await track.readSample()
    if (sample && sample.byteLength >= 4) {
      const frame = stereoRms(sample)
      if (frame.left > frame.right * ratio) {
        return
      }
    }
    await delay(5)
  }
  throw new Error(
    `timed out waiting for ${label}: left RMS did not exceed right * ${ratio} within ${timeoutMs}ms`,
  )
}

/** Wait until inbound stereo is clearly right-dominant (optional probe E gate). */
export async function waitForInboundStereoRightDominant(
  track: StereoEnergyReader,
  options: {
    ratio?: number
    timeoutMs: number
    label?: string
  },
): Promise<void> {
  const {
    ratio = DEFAULT_LOUDER_RATIO,
    timeoutMs,
    label = 'inbound right-dominant stereo',
  } = options
  const endAt = Date.now() + timeoutMs
  while (Date.now() < endAt) {
    const sample = await track.readSample()
    if (sample && sample.byteLength >= 4) {
      const frame = stereoRms(sample)
      if (frame.right > frame.left * ratio) {
        return
      }
    }
    await delay(5)
  }
  throw new Error(
    `timed out waiting for ${label}: right RMS did not exceed left * ${ratio} within ${timeoutMs}ms`,
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
  track: StereoEnergyReader,
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
  track: StereoEnergyReader,
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
  state.ttsQueue.push(Buffer.from(pcm))
  // Same bookkeeping as the wired tee: keeps the TTS stream "active" so a producer that runs a
  // hair slower than 50 Hz gets a skipped slot, not an inserted silence frame.
  state.lastSidecarEnqueueAt = performance.now()
}

/** Loud constant PCM into sidecar queue each 20 ms tick (mirrors wired sidecar tee semantics). */
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

/** Left-only loud PCM into sidecar queue each 20 ms tick (catches pre-downmix pan regressions). */
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
