import { describe, expect, it } from 'vitest'

import {
  FRAME_COUNT,
  SAMPLES_PER_CHANNEL,
  twoSinePannedStereoFrame,
  twoSinePanSidesMatch,
  waitForTwoSinePanSides,
} from './mix-three-client-helpers.js'

function pannedFrames(
  expect440OnRight: boolean,
  count: number,
  phase440 = 0,
  phase880 = 0,
): Buffer[] {
  const frames: Buffer[] = []
  let p440 = phase440
  let p880 = phase880
  for (let i = 0; i < count; i++) {
    frames.push(twoSinePannedStereoFrame(expect440OnRight, 10_000, p440, p880))
    p440 += SAMPLES_PER_CHANNEL
    p880 += SAMPLES_PER_CHANNEL
  }
  return frames
}

function frameReader(frames: Buffer[]): () => Promise<Buffer> {
  let index = 0
  return async () => {
    const frame = frames[index]
    if (frame === undefined) {
      throw new Error('readFrame exhausted')
    }
    index += 1
    return frame
  }
}

function infiniteWrongPanReader(expect440OnRight: boolean): () => Promise<Buffer> {
  let phase440 = 0
  let phase880 = 0
  return async () => {
    const frame = twoSinePannedStereoFrame(!expect440OnRight, 10_000, phase440, phase880)
    phase440 += SAMPLES_PER_CHANNEL
    phase880 += SAMPLES_PER_CHANNEL
    return frame
  }
}

describe('twoSinePanSidesMatch', () => {
  it('matches panned synthetic windows', () => {
    const frames = pannedFrames(true, FRAME_COUNT)
    const left: number[] = []
    const right: number[] = []
    for (const pcm of frames) {
      for (let i = 0; i < SAMPLES_PER_CHANNEL; i++) {
        left.push(pcm.readInt16LE(i * 4))
        right.push(pcm.readInt16LE(i * 4 + 2))
      }
    }
    expect(twoSinePanSidesMatch(Int16Array.from(left), Int16Array.from(right), true)).toBe(true)
    expect(twoSinePanSidesMatch(Int16Array.from(left), Int16Array.from(right), false)).toBe(false)
  })
})

describe('waitForTwoSinePanSides', () => {
  it('discards wrong-pan windows and resolves on correct pan', async () => {
    const expect440OnRight = true
    const frames = [
      ...pannedFrames(!expect440OnRight, FRAME_COUNT * 2),
      ...pannedFrames(expect440OnRight, FRAME_COUNT),
    ]

    const result = await waitForTwoSinePanSides(frameReader(frames), expect440OnRight, 5_000)

    expect(twoSinePanSidesMatch(result.left, result.right, expect440OnRight)).toBe(true)
  })

  it('throws when only wrong-pan frames arrive', async () => {
    await expect(waitForTwoSinePanSides(infiniteWrongPanReader(false), false, 500)).rejects.toThrow(
      /440 Hz should dominate left/,
    )
  })
})
