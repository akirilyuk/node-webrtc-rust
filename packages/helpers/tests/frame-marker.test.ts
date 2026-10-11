import { describe, expect, it } from 'vitest'

import {
  analyseSequence,
  detectIndex,
  FrameMarkerGenerator,
  MARKER_FRAME_BYTES,
  markedFrame,
  type DetectedIndex,
} from './support/frame-marker.js'

function detectRun(indices: number[]): DetectedIndex[] {
  const gen = new FrameMarkerGenerator()
  return indices.map((i) => detectIndex(gen.next(i)))
}

function seq(from: number, count: number): number[] {
  return Array.from({ length: count }, (_, k) => from + k)
}

describe('frame-marker helper', () => {
  it('produces 3840-byte L=R frames', () => {
    const f = markedFrame(7)
    expect(f.length).toBe(MARKER_FRAME_BYTES)
    for (let n = 0; n < 960; n += 97) {
      expect(f.readInt16LE(n * 4)).toBe(f.readInt16LE(n * 4 + 2))
    }
  })

  it('detects every index 0..79 (wraps mod 40) with phase-continuous frames', () => {
    const detected = detectRun(seq(0, 80))
    expect(detected).toEqual(seq(0, 80).map((i) => i % 40))
  })

  it('reports silence for a zero frame', () => {
    expect(detectIndex(Buffer.alloc(MARKER_FRAME_BYTES))).toBe('silence')
  })

  it('perfect sequence across the wrap: no gaps, no duplicates', () => {
    const r = analyseSequence(detectRun(seq(30, 60)))
    expect(r).toMatchObject({ played: 60, gaps: 0, duplicates: 0, ambiguous: 0 })
  })

  it('ignores leading silence and reports firstIndexAt', () => {
    const r = analyseSequence(['silence', 'silence', ...detectRun(seq(5, 10))])
    expect(r.firstIndexAt).toBe(2)
    expect(r).toMatchObject({ played: 10, gaps: 0, duplicates: 0 })
  })

  it('dropping one frame counts one gap', () => {
    const idx = seq(0, 50).filter((i) => i !== 20)
    const r = analyseSequence(detectRun(idx))
    expect(r).toMatchObject({ played: 49, gaps: 1, duplicates: 0 })
  })

  it('dropping one frame at the wrap counts one gap', () => {
    const idx = seq(0, 50).filter((i) => i !== 40)
    expect(analyseSequence(detectRun(idx))).toMatchObject({ gaps: 1, duplicates: 0 })
  })

  it('repeating one frame counts one duplicate', () => {
    const idx = [...seq(0, 20), 19, ...seq(20, 20)]
    const r = analyseSequence(detectRun(idx))
    expect(r).toMatchObject({ gaps: 0, duplicates: 1, played: 40 })
  })

  it('an ambiguous frame stands in for one missing index (no gap)', () => {
    const idx = detectRun(seq(0, 10)) as DetectedIndex[]
    idx[4] = 'ambiguous'
    expect(analyseSequence(idx)).toMatchObject({ played: 9, gaps: 0, duplicates: 0, ambiguous: 1 })
  })

  it('an ambiguous frame explains only one of two missing indices', () => {
    const idx: DetectedIndex[] = [...detectRun(seq(0, 4)), 'ambiguous', ...detectRun([6])]
    expect(analyseSequence(idx)).toMatchObject({ gaps: 1, ambiguous: 1 })
  })

  it('interior silence stands in for one missing index; trailing silence is not interior', () => {
    const idx: DetectedIndex[] = [...detectRun(seq(0, 4)), 'silence', ...detectRun(seq(5, 3))]
    idx.push('silence', 'silence')
    expect(analyseSequence(idx)).toMatchObject({
      gaps: 0,
      interiorSilence: 1,
      silent: 3,
      played: 7,
    })
  })

  it('repeats after the last marker are trailing duplicates, not duplicates', () => {
    const idx = [...seq(0, 10), 9, 9]
    expect(analyseSequence(detectRun(idx))).toMatchObject({
      played: 10,
      duplicates: 0,
      trailingDuplicates: 2,
    })
  })

  it('an index stepping backwards is reordered, not a duplicate', () => {
    const idx = [...seq(0, 10), 5, ...seq(10, 5)]
    expect(analyseSequence(detectRun(idx))).toMatchObject({ reordered: 1, duplicates: 0, gaps: 0 })
  })

  it('a cross-faded merge of frames i and i+1 is ambiguous', () => {
    const gen = new FrameMarkerGenerator()
    const a = gen.next(10)
    const b = gen.next(11)
    const mix = Buffer.alloc(MARKER_FRAME_BYTES)
    for (let n = 0; n < 960; n++) {
      const s = Math.round((a.readInt16LE(n * 4) + b.readInt16LE(n * 4)) / 2)
      mix.writeInt16LE(s, n * 4)
      mix.writeInt16LE(s, n * 4 + 2)
    }
    expect(detectIndex(mix)).toBe('ambiguous')
    const r = analyseSequence([detectIndex(gen.next(9)), detectIndex(mix), ...detectRun([12])])
    expect(r.ambiguous).toBe(1)
  })

  it('survives additive noise and attenuation', () => {
    const gen = new FrameMarkerGenerator()
    let seed = 12345
    const rnd = () => {
      seed = (seed * 1664525 + 1013904223) >>> 0
      return seed / 0xffffffff - 0.5
    }
    for (let i = 0; i < 40; i++) {
      const f = gen.next(i, 0.05)
      for (let n = 0; n < 960; n++) {
        const s = Math.max(
          -32_768,
          Math.min(32_767, f.readInt16LE(n * 4) + Math.round(rnd() * 400)),
        )
        f.writeInt16LE(s, n * 4)
        f.writeInt16LE(s, n * 4 + 2)
      }
      expect(detectIndex(f)).toBe(i)
    }
  })

  // Opus encode/decode survival is exercised by session-pod-mix-frame-continuity.integration.test.ts
  // (the SDK has no standalone Opus encoder usable from unit tests).
})
