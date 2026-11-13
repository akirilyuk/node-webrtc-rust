import { readFileSync } from 'fs'
import { dirname, join } from 'path'
import { fileURLToPath } from 'url'

import { describe, expect, it } from 'vitest'

import type { Seen } from './roundtrip-noisy-bystander-helpers.js'
import {
  ENROLL_PAUSE_MS,
  EXCLUDED_BYSTANDER_VOICES,
  GATE_FALSE_ACCEPT_LEAD_MS,
  benchVoiceLabel,
  enrollPauseAtMs,
  evaluateNoPauseScene,
  inseparableBystanderLabels,
  isExcludedBystander,
  parseCallerPairFar,
  parseSceneFilter,
  sceneSelected,
  shiftSegmentsAfter,
  type NoPauseSegment,
} from './roundtrip-noisy-bystander-nopause-helpers.js'

const here = dirname(fileURLToPath(import.meta.url))
const BENCH = join(here, '../../../crates/vendor-sherpa-onnx/examples/FALSE_ACCEPT_BENCH.md')

const segments: NoPauseSegment[] = [
  {
    role: 'main',
    startMs: 300,
    endMs: 5800,
    text: 'the enrollment sentence is read out slowly',
    kind: 'long',
    enroll: true,
  },
  {
    role: 'bystander',
    startMs: 7300,
    endMs: 12300,
    voicedEndMs: 12100,
    text: 'a passenger asked about the delayed flight to madrid tonight',
    kind: 'background',
  },
  {
    role: 'main',
    startMs: 12200,
    endMs: 14000,
    voicedStartMs: 12300,
    text: 'please check my order',
    kind: 'qualified',
  },
]
/** Scene starts at media ms 1000; `fromStartMs + splitAtMs - 1000` is the scene split point. */
const MEDIA0 = 1000

function start(id: string, atMs: number): Seen {
  return { type: 'user_speaking_start', atMs, utteranceId: id }
}
function final(id: string, atMs: number, speaker: string, text: string): Seen {
  return { type: 'user_speech_final', atMs, utteranceId: id, speaker, text }
}
function barge(id: string, atMs: number): Seen {
  return { type: 'barge_in', atMs, utteranceId: id }
}
function split(
  from: string,
  to: string,
  fromStartScene: number,
  splitAtMs: number,
  labels: [string, string],
): Seen {
  return {
    type: 'user_utterance_split',
    atMs: fromStartScene + splitAtMs + 1500,
    fromUtteranceId: from,
    toUtteranceId: to,
    fromStartMs: fromStartScene + MEDIA0,
    splitAtMs,
    from: labels[0],
    to: labels[1],
  }
}

/** Enrollment u1, bystander u2 split other→caller at scene `splitScene` into u3. */
function clean(splitScene = 12_250, callerText = 'please check my order'): Seen[] {
  return [
    start('u1', 350),
    final('u1', 7000, 'caller', 'the enrollment sentence is read out slowly'),
    start('u2', 7350),
    split('u2', 'u3', 7350, splitScene - 7350, ['other', 'caller']),
    barge('u3', 14_000),
    final('u2', 14_100, 'other', 'a passenger asked about the delayed flight to madrid tonight'),
    final('u3', 15_500, 'caller', callerText),
  ]
}

describe('FALSE_ACCEPT_BENCH lockstep (decision l)', () => {
  const md = readFileSync(BENCH, 'utf8')
  it('parses the amy-low caller rows of the first score column', () => {
    const far = parseCallerPairFar(md)
    expect(far.get('libritts_r 120')).toEqual({ bed: 51.5, clean: 51.5 })
    expect(far.get('joe')).toEqual({ bed: 0, clean: 0 })
    expect(far.size).toBe(5)
  })
  it('excludes exactly the bystander voices with pair FAR >= 5 %', () => {
    const excluded = EXCLUDED_BYSTANDER_VOICES.map((v) => benchVoiceLabel(v.voice, v.sid)).sort()
    expect(excluded).toEqual(inseparableBystanderLabels(md))
  })
  it('matches manifest voices by name and sid', () => {
    expect(benchVoiceLabel('en_US-libritts_r-medium', 120)).toBe('libritts_r 120')
    expect(benchVoiceLabel('en_US-hfc_female-medium', 0)).toBe('hfc_female')
    expect(isExcludedBystander('en_US-libritts_r-medium', 120)).toBe(true)
    expect(isExcludedBystander('en_US-libritts_r-medium', 300)).toBe(false)
    expect(isExcludedBystander('en_US-joe-medium', 0)).toBe(false)
  })
})

describe('evaluateNoPauseScene', () => {
  it('passes a clean other→caller split', () => {
    const r = evaluateNoPauseScene(segments, clean(), 's', MEDIA0)
    expect(r.failures).toEqual([])
    expect(r.otherToCallerSplits).toBe(1)
  })

  it('pairs by id, not by arrival order (caller final first, extra events interleaved)', () => {
    const seen = clean()
    const reordered = [
      seen[0]!,
      seen[1]!,
      seen[2]!,
      seen[6]!,
      seen[3]!,
      start('u9', 9000),
      seen[5]!,
      seen[4]!,
    ]
    const r = evaluateNoPauseScene(segments, reordered, 's', MEDIA0)
    expect(r.failures).toEqual([])
  })

  it('fails a caller final with bystander words after a cut a caller window confirmed', () => {
    const r = evaluateNoPauseScene(
      segments,
      clean(11_000, 'madrid tonight please check my order'),
      's',
      MEDIA0,
    )
    expect(r.cutPointLeaks.map((l) => l.words)).toEqual([['madrid', 'tonight']])
    expect(r.failures.some((f) => f.includes('holds bystander words'))).toBe(true)
    expect(r.gateFalseAcceptLeaks).toEqual([])
  })

  it('fails leading glue (a short bystander word before the caller phrase)', () => {
    const md = segments.map((s) =>
      s.role === 'bystander' ? { ...s, text: 'she said it was on' } : s,
    )
    const r = evaluateNoPauseScene(md, clean(12_250, 'on please check my order'), 's', MEDIA0)
    expect(r.leadingGlue.map((l) => l.words)).toEqual([['on']])
    expect(r.failures.some((f) => f.includes('leading glue'))).toBe(true)
  })

  it('reports a leak after a gate false accept as its own class without failing', () => {
    const at = 12_300 - GATE_FALSE_ACCEPT_LEAD_MS - 1
    const r = evaluateNoPauseScene(
      segments,
      clean(at, 'flight to madrid tonight please check my order'),
      's',
      MEDIA0,
    )
    expect(r.gateFalseAcceptLeaks.map((l) => l.words)).toEqual([['flight', 'madrid', 'tonight']])
    expect(r.failures).toEqual([])
  })

  it('a split exactly at the false-accept bound is a cut-point leak', () => {
    const at = 12_300 - GATE_FALSE_ACCEPT_LEAD_MS
    const r = evaluateNoPauseScene(segments, clean(at, 'flight please check my order'), 's', MEDIA0)
    expect(r.cutPointLeaks).toHaveLength(1)
    expect(r.failures).toHaveLength(1)
  })

  it('fails the upgrade fallback: no split, the bystander utterance ends as caller', () => {
    const seen: Seen[] = [
      start('u1', 350),
      final('u1', 7000, 'caller', 'the enrollment sentence is read out slowly'),
      start('u2', 7350),
      barge('u2', 14_000),
      final('u2', 15_500, 'caller', 'the delayed flight to madrid tonight please check my order'),
    ]
    const r = evaluateNoPauseScene(segments, seen, 's', MEDIA0)
    expect(r.cutPointLeaks).toHaveLength(1)
    expect(r.failures.some((f) => f.includes('holds bystander words'))).toBe(true)
  })

  it('fails when the caller gets no utterance of its own', () => {
    const seen: Seen[] = [
      start('u1', 350),
      final('u1', 7000, 'caller', 'the enrollment sentence is read out slowly'),
      start('u2', 7350),
      final('u2', 15_500, 'other', 'the delayed flight to madrid tonight please check my order'),
    ]
    const r = evaluateNoPauseScene(segments, seen, 's', MEDIA0)
    expect(r.failures.some((f) => f.includes('no caller utterance of its own'))).toBe(true)
  })

  it('fails a barge-in on the bystander part and a missing caller barge-in', () => {
    const seen = clean().map((e) => (e.type === 'barge_in' ? { ...e, utteranceId: 'u2' } : e))
    const r = evaluateNoPauseScene(segments, seen, 's', MEDIA0)
    expect(r.failures.some((f) => f.includes('whose final is speaker other'))).toBe(true)
    expect(r.failures.some((f) => f.includes('no barge_in on caller utterance u3'))).toBe(true)
  })

  it('follows a re-cut chain to the caller utterance that ends in a final', () => {
    const seen: Seen[] = [
      start('u1', 350),
      final('u1', 7000, 'caller', 'the enrollment sentence is read out slowly'),
      start('u2', 7350),
      split('u2', 'u3', 7350, 1000, ['other', 'caller']),
      split('u3', 'u4', 8350, 1500, ['caller', 'other']),
      split('u4', 'u5', 9850, 2400, ['other', 'caller']),
      barge('u5', 14_000),
      final('u2', 9000, 'other', 'a passenger'),
      final('u3', 10_000, 'caller', 'asked'),
      final('u4', 13_000, 'other', 'about the delayed'),
      final('u5', 15_500, 'caller', 'please check my order'),
    ]
    const r = evaluateNoPauseScene(segments, seen, 's', MEDIA0)
    expect(r.failures).toEqual([])
    expect(r.otherToCallerSplits).toBe(2)
  })

  it('keeps the caller utterance when trailing bed is split off caller→other (run 20261113-223647 n03)', () => {
    const seen: Seen[] = [
      ...clean(),
      split('u3', 'u4', 12_250, 1800, ['caller', 'other']),
      final('u4', 17_000, 'other', ''),
    ]
    const r = evaluateNoPauseScene(segments, seen, 's', MEDIA0)
    expect(r.failures).toEqual([])
  })

  it('fails a qualified caller phrase below the similarity threshold', () => {
    const r = evaluateNoPauseScene(segments, clean(12_250, 'please'), 's', MEDIA0)
    expect(r.failures.some((f) => f.includes('similarity'))).toBe(true)
  })
})

describe('enrollment pause time shift', () => {
  it('moves segments at or after the insert by exactly the inserted ms, earlier ones stay', () => {
    const at = enrollPauseAtMs(segments)!
    expect(at).toBe(6000)
    const out = shiftSegmentsAfter(segments, at, ENROLL_PAUSE_MS)
    expect(out[0]).toEqual(segments[0])
    expect(out[1]!.startMs).toBe(7300 + ENROLL_PAUSE_MS)
    expect(out[1]!.endMs).toBe(12_300 + ENROLL_PAUSE_MS)
    expect(out[1]!.voicedEndMs).toBe(12_100 + ENROLL_PAUSE_MS)
    expect(out[1]!.voicedStartMs).toBeUndefined()
    expect(out[2]!.voicedStartMs).toBe(12_300 + ENROLL_PAUSE_MS)
    expect(out[2]!.startMs - out[1]!.startMs).toBe(segments[2]!.startMs - segments[1]!.startMs)
  })
  it('a segment starting exactly at the insert point moves', () => {
    const out = shiftSegmentsAfter(segments, 7300, 100)
    expect(out[1]!.startMs).toBe(7400)
    expect(out[0]!.startMs).toBe(300)
  })
})

describe('SHERPA_NOPAUSE_ONLY scene filter', () => {
  it('plays every scene when unset or blank', () => {
    expect(parseSceneFilter(undefined)).toBeUndefined()
    expect(parseSceneFilter(' , ')).toBeUndefined()
    expect(sceneSelected('s4_test_n02', undefined)).toBe(true)
  })
  it('keeps only the listed ids, exact match', () => {
    const f = parseSceneFilter('s4_test_n02, s4_test_n05')
    expect(sceneSelected('s4_test_n02', f)).toBe(true)
    expect(sceneSelected('s4_test_n05', f)).toBe(true)
    expect(sceneSelected('s4_test_n0', f)).toBe(false)
    expect(sceneSelected('s4_test_n03', f)).toBe(false)
  })
})
