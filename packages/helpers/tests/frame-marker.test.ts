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

  describe('a forward step of more than half the period (mod-40 aliasing)', () => {
    /** Markers of source frames `from..from+count-1`, as the detector reports them. */
    const marks = (from: number, count: number): DetectedIndex[] =>
      seq(from, count).map((i) => i % 40)
    const ambiguous = (n: number): DetectedIndex[] => Array.from({ length: n }, () => 'ambiguous')

    // Shape of a stalled run (mixer stats: compressed 11): frames 0..20 heard, 11 merged
    // (ambiguous) outputs that together carry source frames 21..82, then frame 83 (marker 3).
    // The step 20 -> 3 is 23 mod 40, which is also a backward step of 17.
    const stalled = [...marks(0, 21), ...ambiguous(11), ...marks(83, 31)]

    it('is forward when the merges the mixer reports pay for it', () => {
      const r = analyseSequence(stalled, { mergedFrames: 11 })
      expect(r).toMatchObject({
        reordered: 0,
        gaps: 11,
        duplicates: 0,
        ambiguous: 11,
        played: 52,
      })
    })

    it('is reordered when no merge is accounted for', () => {
      // 17 frames (3..19) read as backward steps of 17..1 until the marker wraps.
      expect(analyseSequence(stalled).reordered).toBe(17)
      expect(analyseSequence(stalled, { mergedFrames: 10 }).reordered).toBeGreaterThan(0)
    })

    it('spends the merge budget: a second long step needs its own merges', () => {
      const twice = [...stalled, ...ambiguous(11), ...marks(83 + 31 + 22, 10)]
      expect(analyseSequence(twice, { mergedFrames: 22 })).toMatchObject({ reordered: 0, gaps: 22 })
      expect(analyseSequence(twice, { mergedFrames: 11 }).reordered).toBeGreaterThan(0)
    })

    it('an adjacent swap is still reordered with a large merge budget', () => {
      const idx = [...marks(0, 10), 11, 10, ...marks(12, 9)]
      expect(analyseSequence(idx, { mergedFrames: 100 })).toMatchObject({
        reordered: 1,
        gaps: 1,
        duplicates: 0,
      })
    })

    it('a long run of unknown frames explains the step by itself', () => {
      // 25 unknown frames then marker 5 + 26: forward, with no merge needed.
      const idx = [...marks(0, 6), ...ambiguous(25), ...marks(31, 5)]
      expect(analyseSequence(idx)).toMatchObject({ reordered: 0, gaps: 0, ambiguous: 25 })
    })

    describe('listener catch-up merges stacked on queue compressions', () => {
      /** Parses a recorded `SEQ` dump: numbers, `A` = ambiguous, `.` = silence. */
      const parse = (s: string): DetectedIndex[] =>
        s
          .trim()
          .split(/\s+/)
          .map((t) => (t === 'A' ? 'ambiguous' : t === '.' ? 'silence' : Number(t)))

      // Recorded with MIX_FC_DUMP=1: the idle-gap case sent with a 300 ms sender stall while the
      // test process was SIGSTOP duty-cycled (150 ms stopped / 50 ms running), the same
      // signature as CI job 122629532201 (reordered 15, duplicates 1). Mixer stats: compressed
      // 11, listenerCatchupCompressed 40, concealed 2. After 18, 11 ambiguous frames carry
      // source frames up to 86 (marker 6): delta 28, 16 indices beyond the unknown frames, more
      // than one per frame because a catch-up frame merges two ticks that were each compressed.
      const recorded = parse(
        '. . A 0 1 2 3 4 5 6 7 8 9 A A A A A 20 21 22 23 24 25 26 27 28 29 30 31 A A A A A 2 ' +
          '3 4 5 6 7 8 9 A . . . . . . . A 10 A 13 14 15 16 17 18 A A A A A A A A A A A 6 7 8 9 ' +
          '10 11 12 13 14 15 16 17 18 A A A A A 29 30 31 32 33 34 35 36 37 38 A 0 A A A A A 11 ' +
          '12 13 14 15 16 17 18 19 20 21 22 A A A A . . . . . . . . . . . . . . . .',
      )
      const stats = { mergedFrames: 11 + 40, listenerCatchupFrames: 40 }

      it('reads the recorded run as forward when both merge kinds are reported', () => {
        const r = analyseSequence(recorded, stats)
        expect(r.reordered).toBe(0)
        expect(r.duplicates).toBe(0)
        expect(r.gaps).toBeLessThanOrEqual(stats.mergedFrames)
      })

      it('misread it as 12 backward steps and a duplicate before (one index per frame)', () => {
        expect(analyseSequence(recorded, { mergedFrames: stats.mergedFrames })).toMatchObject({
          reordered: 12,
          duplicates: 1,
        })
      })

      it('catch-up merges alone still allow one index per frame', () => {
        // 20 -> 42 (marker 2) over 5 unknown frames: 16 indices to pay for, more than one per
        // frame. No queue compression: a catch-up frame merges two plain ticks, absorbing one.
        const idx = [...marks(0, 21), ...ambiguous(5), ...marks(42, 10)]
        expect(
          analyseSequence(idx, { mergedFrames: 40, listenerCatchupFrames: 40 }).reordered,
        ).toBeGreaterThan(0)
        expect(analyseSequence(idx, { mergedFrames: 41, listenerCatchupFrames: 40 })).toMatchObject(
          { reordered: 0, gaps: 16 },
        )
      })

      it('an adjacent swap is still reordered with stacked merges', () => {
        const idx = [...marks(0, 10), 11, 10, ...marks(12, 9)]
        expect(
          analyseSequence(idx, { mergedFrames: 100, listenerCatchupFrames: 50 }).reordered,
        ).toBe(1)
      })
    })
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
