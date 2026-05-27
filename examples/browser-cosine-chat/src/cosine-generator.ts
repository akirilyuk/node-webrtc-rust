/** PCM layout used by {@link LocalAudioTrack.writeSample} (48 kHz, stereo, 16-bit). */
export const PCM_SAMPLE_RATE = 48_000
export const PCM_CHANNELS = 2
export const PCM_FRAME_DURATION_MS = 20

/** Browser ontrack kick — quarter frame; pass `durationMs: 5` to writeSample. */
export const PCM_KICK_FRAME_BYTES = 960
export const PCM_KICK_DURATION_MS = 5

export interface CosineGeneratorOptions {
  /** Tone frequency in hertz (default 440 Hz — concert A). */
  frequencyHz?: number
  /** Peak amplitude as a fraction of full scale, 0–1 (default 0.25). */
  amplitude?: number
  sampleRate?: number
  channels?: number
  frameDurationMs?: number
}

/** Generates interleaved stereo 16-bit PCM frames shaped as a cosine (sinusoid). */
export class CosineGenerator {
  readonly frequencyHz: number
  readonly amplitude: number
  readonly sampleRate: number
  readonly channels: number
  readonly frameDurationMs: number
  private phase = 0

  constructor(options: CosineGeneratorOptions = {}) {
    this.frequencyHz = options.frequencyHz ?? 440
    this.amplitude = options.amplitude ?? 0.25
    this.sampleRate = options.sampleRate ?? PCM_SAMPLE_RATE
    this.channels = options.channels ?? PCM_CHANNELS
    this.frameDurationMs = options.frameDurationMs ?? PCM_FRAME_DURATION_MS
  }

  /** Number of PCM samples per channel in one frame. */
  samplesPerFrame(): number {
    return Math.round(this.sampleRate * (this.frameDurationMs / 1000))
  }

  /** Byte length of one interleaved PCM frame. */
  frameByteLength(): number {
    return this.samplesPerFrame() * this.channels * 2
  }

  /** Returns the next PCM frame and advances the oscillator phase. */
  nextFrame(): Buffer {
    const samples = this.samplesPerFrame()
    const buffer = Buffer.alloc(samples * this.channels * 2)
    const phaseStep = (2 * Math.PI * this.frequencyHz) / this.sampleRate

    for (let i = 0; i < samples; i++) {
      const value = Math.round(this.amplitude * 32767 * Math.sin(this.phase))
      this.phase += phaseStep

      for (let channel = 0; channel < this.channels; channel++) {
        buffer.writeInt16LE(value, (i * this.channels + channel) * 2)
      }
    }

    return buffer
  }
}
