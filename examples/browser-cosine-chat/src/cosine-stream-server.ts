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
  private timer: ReturnType<typeof setTimeout> | null = null
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
    void this.writeFrame(track)
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
    this.scheduleTick()
  }

  /** Stops generating frames. */
  stop(): void {
    if (this.timer) {
      clearTimeout(this.timer)
      this.timer = null
    }
    this.running = false
    this.tracks.clear()
  }

  private scheduleTick(): void {
    if (!this.running) return
    this.timer = setTimeout(() => {
      void this.runTick()
    }, PCM_FRAME_DURATION_MS)
  }

  private async runTick(): Promise<void> {
    if (!this.running) return

    const tracks = [...this.tracks]
    for (const track of tracks) {
      await this.writeFrame(track)
    }

    this.scheduleTick()
  }

  private async writeFrame(track: LocalAudioTrack): Promise<void> {
    const frame = this.generator.nextFrame()
    try {
      // Copy per track — writeSample may retain the buffer while encoding.
      await track.writeSample(Buffer.from(frame), PCM_FRAME_DURATION_MS)
    } catch (error: unknown) {
      console.error('CosineStreamServer: writeSample failed', error)
    }
  }
}
