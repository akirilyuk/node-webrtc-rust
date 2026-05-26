import type { LocalAudioTrack } from '@node-webrtc-rust/sdk'

import { CosineGenerator, PCM_FRAME_DURATION_MS } from './cosine-generator'

export interface CosineStreamServerOptions {
  frequencyHz?: number
  amplitude?: number
}

/**
 * Server-side cosine tone broadcaster.
 *
 * Generates PCM frames once and fans them out to every subscribed
 * {@link LocalAudioTrack}. Call {@link primeTrack} before waiting for remote
 * `ontrack` — the first {@link LocalAudioTrack.writeSample} triggers it.
 */
export class CosineStreamServer {
  readonly generator: CosineGenerator
  private readonly tracks = new Set<LocalAudioTrack>()
  private timer: ReturnType<typeof setInterval> | null = null
  private running = false

  constructor(options: CosineStreamServerOptions = {}) {
    this.generator = new CosineGenerator({
      frequencyHz: options.frequencyHz,
      amplitude: options.amplitude,
    })
  }

  /** Adds a track that receives every generated frame. */
  subscribe(track: LocalAudioTrack): () => void {
    this.tracks.add(track)
    return () => {
      this.tracks.delete(track)
    }
  }

  /** Writes one frame to a single track (use before awaiting remote `ontrack`). */
  async primeTrack(track: LocalAudioTrack): Promise<void> {
    await track.writeSample(this.generator.nextFrame(), PCM_FRAME_DURATION_MS)
  }

  /** Starts the periodic frame loop for all subscribed tracks. */
  start(): void {
    if (this.running) return
    this.running = true

    this.timer = setInterval(() => {
      void this.tick()
    }, PCM_FRAME_DURATION_MS)
  }

  /** Stops generating frames. */
  stop(): void {
    if (this.timer) {
      clearInterval(this.timer)
      this.timer = null
    }
    this.running = false
    this.tracks.clear()
  }

  private async tick(): Promise<void> {
    if (this.tracks.size === 0) return

    const frame = this.generator.nextFrame()
    await Promise.all(
      [...this.tracks].map((track) => track.writeSample(frame, PCM_FRAME_DURATION_MS)),
    )
  }
}
