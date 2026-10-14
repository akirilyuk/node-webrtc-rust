import { describe, expect, it } from 'vitest'

import { PCM_FULL_FRAME_BYTES } from '../../shared/pcm-streaming.js'
import {
  countOrderedNumberWords,
  createNoiseFrames,
  evaluatePhaseA,
  evaluatePhaseB,
  evaluatePhaseC,
  type C1SmokeEvent,
} from './roundtrip-c1-timeouts-helpers.js'

const notFound = (atMs: number): C1SmokeEvent => ({ type: 'user_stt_not_found', atMs })
const final = (atMs: number, text: string): C1SmokeEvent => ({
  type: 'user_speech_final',
  atMs,
  text,
})

describe('createNoiseFrames', () => {
  it('is deterministic, frame-sized and loud enough for the energy VAD', () => {
    const a = createNoiseFrames(600, 7)
    const b = createNoiseFrames(600, 7)
    expect(a).toHaveLength(30)
    expect(a.every((f) => f.length === PCM_FULL_FRAME_BYTES)).toBe(true)
    expect(a.every((f, i) => f.equals(b[i]!))).toBe(true)
    expect(createNoiseFrames(600, 8)[0]!.equals(a[0]!)).toBe(false)
    let sum = 0
    let n = 0
    for (const f of a) {
      for (let i = 0; i < f.length; i += 4) {
        const s = f.readInt16LE(i) / 32768
        sum += s * s
        n += 1
      }
    }
    expect(Math.sqrt(sum / n)).toBeGreaterThan(0.05)
  })
})

describe('evaluatePhaseA', () => {
  it('passes with one not_found 2342 ms after the noise end (normal endpoint close)', () => {
    const r = evaluatePhaseA({ events: [notFound(2942)], noiseEndAtMs: 600 })
    expect(r.passed).toBe(true)
    expect(r.notFoundAfterMs).toEqual([2342])
    expect(evaluatePhaseA({ events: [notFound(4600)], noiseEndAtMs: 600 }).passed).toBe(true)
  })

  it('fails when not_found fires 1000 ms after the noise end', () => {
    const r = evaluatePhaseA({ events: [notFound(1600)], noiseEndAtMs: 600 })
    expect(r.passed).toBe(false)
    expect(r.failures.join(' ')).toContain('1000 ms')
  })

  it('fails when not_found fires 7000 ms after the noise end', () => {
    expect(evaluatePhaseA({ events: [notFound(7600)], noiseEndAtMs: 600 }).passed).toBe(false)
  })

  it('fails with no not_found, two not_found, or a non-empty final', () => {
    expect(evaluatePhaseA({ events: [], noiseEndAtMs: 600 }).passed).toBe(false)
    expect(
      evaluatePhaseA({ events: [notFound(4600), notFound(5000)], noiseEndAtMs: 600 }).passed,
    ).toBe(false)
    expect(
      evaluatePhaseA({ events: [notFound(4600), final(4700, 'hello')], noiseEndAtMs: 600 }).passed,
    ).toBe(false)
    expect(
      evaluatePhaseA({ events: [notFound(4600), final(4700, '  ')], noiseEndAtMs: 600 }).passed,
    ).toBe(true)
  })
})

describe('evaluatePhaseB', () => {
  it('passes with one not_found 12 s after the stop', () => {
    const r = evaluatePhaseB({ events: [notFound(12_400)], stopAtMs: 400 })
    expect(r.passed).toBe(true)
  })

  it('fails when not_found fires 5 s after the stop', () => {
    const r = evaluatePhaseB({ events: [notFound(5400)], stopAtMs: 400 })
    expect(r.passed).toBe(false)
    expect(r.failures.join(' ')).toContain('earlier than')
  })

  it('fails when none arrives by 15 s or when two arrive', () => {
    expect(evaluatePhaseB({ events: [notFound(16_000)], stopAtMs: 400 }).passed).toBe(false)
    expect(evaluatePhaseB({ events: [], stopAtMs: 400 }).passed).toBe(false)
    expect(
      evaluatePhaseB({ events: [notFound(12_400), notFound(13_000)], stopAtMs: 400 }).passed,
    ).toBe(false)
  })
})

describe('evaluatePhaseC', () => {
  const text = 'one two three four five six seven eight nine ten'

  it('passes with a full final and no not_found', () => {
    expect(evaluatePhaseC({ events: [final(9000, text)] }).passed).toBe(true)
  })

  it('passes with finals joined across the gap', () => {
    const r = evaluatePhaseC({
      events: [final(3000, 'one two three four'), final(9000, 'five six seven eight nine')],
    })
    expect(r.passed).toBe(true)
  })

  it('fails on any not_found in the phase', () => {
    const r = evaluatePhaseC({ events: [notFound(5000), final(9000, text)] })
    expect(r.passed).toBe(false)
    expect(r.failures.join(' ')).toContain('user_stt_not_found')
  })

  it('fails when only the words before the gap were heard', () => {
    const r = evaluatePhaseC({ events: [final(3000, 'one two three four five six')] })
    expect(r.passed).toBe(false)
    expect(r.failures.length).toBe(2)
  })
})

describe('countOrderedNumberWords', () => {
  it('counts in-order words and accepts digits', () => {
    expect(countOrderedNumberWords('ONE, TWO 3 four')).toEqual({ count: 4, lateWord: false })
    expect(countOrderedNumberWords('one two nine')).toEqual({ count: 3, lateWord: true })
    expect(countOrderedNumberWords('three two one').count).toBe(1)
  })
})
