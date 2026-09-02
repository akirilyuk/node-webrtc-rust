export const SAMPLE_RATE = 48_000
export const SAMPLES_PER_CHANNEL = 960
export const FRAME_BYTES = 3_840
export const FRAME_COUNT = 15
export const MIN_RATIO = 2

export function sineStereoFrame(freqHz: number, amplitude: number, phase: number): Buffer {
  const pcm = Buffer.alloc(FRAME_BYTES)
  for (let i = 0; i < SAMPLES_PER_CHANNEL; i++) {
    const t = (phase + i) / SAMPLE_RATE
    const sample = Math.round(amplitude * Math.sin(2 * Math.PI * freqHz * t))
    const clamped = Math.max(-32_767, Math.min(32_767, sample))
    const base = i * 4
    pcm.writeInt16LE(clamped, base)
    pcm.writeInt16LE(clamped, base + 2)
  }
  return pcm
}

export function extractChannel(pcm: Buffer, channel: 0 | 1): Int16Array {
  const out = new Int16Array(SAMPLES_PER_CHANNEL)
  for (let i = 0; i < SAMPLES_PER_CHANNEL; i++) {
    out[i] = pcm.readInt16LE(i * 4 + channel * 2)
  }
  return out
}

export function goertzelPower(samples: Int16Array, targetFreq: number): number {
  const n = samples.length
  if (n === 0) {
    return 0
  }
  const k = Math.floor(0.5 + (n * targetFreq) / SAMPLE_RATE)
  const omega = (2 * Math.PI * k) / n
  const coeff = 2 * Math.cos(omega)
  let s0 = 0
  let s1 = 0
  let s2 = 0
  for (let i = 0; i < n; i++) {
    const x = samples[i]!
    s0 = x + coeff * s1 - s2
    s2 = s1
    s1 = s0
  }
  return s1 * s1 + s2 * s2 - coeff * s1 * s2
}

export function assertTwoSinePanSides(
  left: Int16Array,
  right: Int16Array,
  expect440OnRight: boolean,
): void {
  const p440L = goertzelPower(left, 440)
  const p440R = goertzelPower(right, 440)
  const p880L = goertzelPower(left, 880)
  const p880R = goertzelPower(right, 880)

  if (expect440OnRight) {
    if (p440R < MIN_RATIO * p440L) {
      throw new Error(`440 Hz should dominate right (L=${p440L}, R=${p440R})`)
    }
    if (p440R < MIN_RATIO * p880R) {
      throw new Error(`440 Hz on right should beat 880 Hz on right (440R=${p440R}, 880R=${p880R})`)
    }
    if (p880L < MIN_RATIO * p880R) {
      throw new Error(`880 Hz should dominate left (L=${p880L}, R=${p880R})`)
    }
    if (p880L < MIN_RATIO * p440L) {
      throw new Error(`880 Hz on left should beat 440 Hz on left (880L=${p880L}, 440L=${p440L})`)
    }
  } else {
    if (p440L < MIN_RATIO * p440R) {
      throw new Error(`440 Hz should dominate left after pose swap (L=${p440L}, R=${p440R})`)
    }
    if (p440L < MIN_RATIO * p880L) {
      throw new Error(`440 Hz on left should beat 880 Hz on left (440L=${p440L}, 880L=${p880L})`)
    }
    if (p880R < MIN_RATIO * p880L) {
      throw new Error(`880 Hz should dominate right after pose swap (L=${p880L}, R=${p880R})`)
    }
    if (p880R < MIN_RATIO * p440R) {
      throw new Error(`880 Hz on right should beat 440 Hz on right (880R=${p880R}, 440R=${p440R})`)
    }
  }
}

export function appendStereoChannels(left: number[], right: number[], pcm: Buffer): void {
  left.push(...extractChannel(pcm, 0))
  right.push(...extractChannel(pcm, 1))
}
