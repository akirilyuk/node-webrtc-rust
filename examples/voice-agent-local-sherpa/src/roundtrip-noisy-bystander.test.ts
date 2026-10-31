import { describe, expect, it } from 'vitest'

import {
  callerWindowSimilarity,
  evaluateTurns,
  expectedBargeAtMs,
  matchTurnUtterances,
  pairTurnUtterances,
  sttPartialTokenCount,
  type Seen,
  type Turn,
} from './roundtrip-noisy-bystander-helpers.js'

const caller: Turn = {
  role: 'main',
  startMs: 10_000,
  endMs: 12_000,
  text: 'please check my order',
  kind: 'qualified',
}
const bystander: Turn = {
  role: 'bystander',
  startMs: 14_000,
  endMs: 16_000,
  text: 'the weather is nice today',
  kind: 'background',
}

const bystander2: Turn = {
  role: 'bystander',
  startMs: 18_000,
  endMs: 20_000,
  text: 'a bus waits outside',
  kind: 'background',
}
const bystander3: Turn = {
  role: 'bystander',
  startMs: 22_000,
  endMs: 24_000,
  text: 'the light is green',
  kind: 'background',
}
const TURNS = [caller, bystander, bystander2, bystander3]

function ev(type: string, atMs: number, extra: Partial<Seen> = {}): Seen {
  return { type, atMs, ...extra }
}

/** A healthy pass: caller barges 1.4 s after speech start, bystander is flagged other. */
function healthy(): Seen[] {
  return [
    ev('user_speaking_start', 10_100, { utteranceId: 'u1' }),
    ev('user_speech_partial', 11_400, { utteranceId: 'u1', text: 'please check' }),
    ev('barge_in', 11_500, { utteranceId: 'u1' }),
    ev('user_speech_final', 13_000, {
      utteranceId: 'u1',
      text: 'please check my order',
      speaker: 'caller',
    }),
    ev('user_speaking_start', 14_100, { utteranceId: 'u2' }),
    ev('user_speech_final', 17_000, { utteranceId: 'u2', text: 'the weather', speaker: 'other' }),
    ev('user_speaking_start', 18_100, { utteranceId: 'u3' }),
    ev('user_speech_final', 21_000, { utteranceId: 'u3', text: 'a bus', speaker: 'other' }),
    ev('user_speaking_start', 22_100, { utteranceId: 'u4' }),
    ev('user_speech_final', 25_000, { utteranceId: 'u4', text: 'light', speaker: 'other' }),
  ]
}

describe('evaluateTurns', () => {
  it('passes a healthy pass', () => {
    expect(evaluateTurns(TURNS, healthy(), 'p')).toEqual([])
  })

  it('fails a barge-in that carries the bystander utterance id', () => {
    const seen = [...healthy(), ev('barge_in', 15_000, { utteranceId: 'u2' })]
    expect(evaluateTurns(TURNS, seen, 'p').join('\n')).toMatch(
      /barge_in for the bystander utterance u2/,
    )
  })

  it('fails a barge on an unmatched utterance labelled other', () => {
    const seen = [
      ...healthy(),
      ev('user_speaking_start', 15_000, { utteranceId: 'u3' }),
      ev('barge_in', 15_200, { utteranceId: 'u3' }),
      ev('user_speech_final', 16_500, { utteranceId: 'u3', text: 'more noise', speaker: 'other' }),
    ]
    expect(evaluateTurns(TURNS, seen, 'p').join('\n')).toMatch(/u3 whose final is speaker other/)
  })

  it('attributes a late barge of the previous caller utterance to that utterance', () => {
    // Inside the bystander's time window, but the id says it is the caller's utterance u1.
    const seen = [...healthy(), ev('barge_in', 14_500, { utteranceId: 'u1' })]
    seen.sort((a, b) => a.atMs - b.atMs)
    expect(evaluateTurns(TURNS, seen, 'p')).toEqual([])
  })

  it('fails a barge_in without utteranceId', () => {
    const seen = [...healthy(), ev('barge_in', 12_500)]
    expect(evaluateTurns(TURNS, seen, 'p').join('\n')).toMatch(/without utteranceId/)
  })

  it('matches turns to utterances by the first speaking start inside the turn slot', () => {
    // u2 starts 2.3 s after the bystander's scene start (pipeline delay), still before any next turn.
    const seen = healthy().map((e) =>
      e.type === 'user_speaking_start' && e.utteranceId === 'u2' ? { ...e, atMs: 16_300 } : e,
    )
    expect(matchTurnUtterances(TURNS, seen).map((s) => s?.utteranceId)).toEqual([
      'u1',
      'u2',
      'u3',
      'u4',
    ])
    // VAD opened in the babble 700 ms before the bystander voice (caller ended at 12 000, slot
    // boundary 13 000): still the bystander's utterance.
    const early = healthy().map((e) =>
      e.type === 'user_speaking_start' && e.utteranceId === 'u2' ? { ...e, atMs: 13_300 } : e,
    )
    expect(matchTurnUtterances(TURNS, early).map((s) => s?.utteranceId)).toEqual([
      'u1',
      'u2',
      'u3',
      'u4',
    ])
    // A bystander merged into the caller utterance has no own start: nothing matched, no steal.
    const merged = healthy().filter((e) => e.utteranceId !== 'u2')
    expect(matchTurnUtterances(TURNS, merged).map((s) => s?.utteranceId)).toEqual([
      'u1',
      undefined,
      'u3',
      'u4',
    ])
  })

  it('fails a bystander final tagged caller', () => {
    const seen = healthy().map((e) =>
      e.utteranceId === 'u2' && e.speaker ? { ...e, speaker: 'caller' } : e,
    )
    expect(evaluateTurns(TURNS, seen, 'p').join('\n')).toMatch(/expected other/)
  })

  it('bounds the barge by the verdict when a qualifying partial came first', () => {
    // Start 10 100, qualifying partial 10 600 -> expected = verdict 11 600.
    const base = healthy().filter((e) => e.type !== 'user_speech_partial' && e.type !== 'barge_in')
    const seen = [
      ...base,
      ev('user_speech_partial', 10_600, { utteranceId: 'u1', text: 'please check' }),
      ev('barge_in', 11_650, { utteranceId: 'u1' }),
    ].sort((a, b) => a.atMs - b.atMs)
    expect(evaluateTurns(TURNS, seen, 'p')).toEqual([])
    expect(expectedBargeAtMs(seen[0]!, seen)).toBe(11_600)
  })

  it('passes a barge at the first qualifying partial after a 1-token partial', () => {
    const base = healthy().filter((e) => e.type !== 'user_speech_partial' && e.type !== 'barge_in')
    const seen = [
      ...base,
      ev('user_speech_partial', 11_000, { utteranceId: 'u1', text: 'please' }),
      ev('user_speech_partial', 12_270, { utteranceId: 'u1', text: 'please check my' }),
      ev('barge_in', 12_280, { utteranceId: 'u1' }),
    ].sort((a, b) => a.atMs - b.atMs)
    expect(evaluateTurns(TURNS, seen, 'p')).toEqual([])
  })

  it('accepts a re-checked caller barge after the 3 s cap, bounded by start + 2 x checkSpeechMs', () => {
    // Start 10 100, re-checked: verdict 10 100 + 3 000 = 13 100; barge 13 300 (+200 ms, 3 200 ms delay).
    const base = healthy().filter(
      (e) => e.type !== 'user_speech_partial' && e.type !== 'barge_in' && e.utteranceId !== 'u1',
    )
    const seen = [
      ...base,
      ev('user_speaking_start', 10_100, { utteranceId: 'u1' }),
      ev('user_speech_partial', 10_600, { utteranceId: 'u1', text: 'please check' }),
      ev('barge_in', 13_300, { utteranceId: 'u1' }),
      ev('user_speech_final', 13_500, {
        utteranceId: 'u1',
        text: 'please check my order',
        speaker: 'caller',
        speakerRechecked: true,
      }),
    ].sort((a, b) => a.atMs - b.atMs)
    expect(evaluateTurns(TURNS, seen, 'p')).toEqual([])
    expect(expectedBargeAtMs(seen.find((e) => e.utteranceId === 'u1')!, seen)).toBe(13_100)
  })

  it('fails a re-checked caller barge more than 300 ms after start + 2 x checkSpeechMs', () => {
    const base = healthy().filter(
      (e) => e.type !== 'user_speech_partial' && e.type !== 'barge_in' && e.utteranceId !== 'u1',
    )
    const seen = [
      ...base,
      ev('user_speaking_start', 10_100, { utteranceId: 'u1' }),
      ev('user_speech_partial', 10_600, { utteranceId: 'u1', text: 'please check' }),
      ev('barge_in', 13_500, { utteranceId: 'u1' }),
      ev('user_speech_final', 13_600, {
        utteranceId: 'u1',
        text: 'please check my order',
        speaker: 'caller',
        speakerRechecked: true,
      }),
    ].sort((a, b) => a.atMs - b.atMs)
    expect(evaluateTurns(TURNS, seen, 'p').join('\n')).toMatch(/400 ms after expected .* > 300/)
  })

  it('keeps the 3 s cap for a caller turn that was not re-checked', () => {
    const base = healthy().filter((e) => e.type !== 'user_speech_partial' && e.type !== 'barge_in')
    const seen = [
      ...base,
      ev('user_speech_partial', 13_000, { utteranceId: 'u1', text: 'please check' }),
      ev('barge_in', 13_200, { utteranceId: 'u1' }),
    ].sort((a, b) => a.atMs - b.atMs)
    expect(evaluateTurns(TURNS, seen, 'p').join('\n')).toMatch(/> cap 3000/)
  })

  it('fails a bystander upgraded to caller by a re-check, with its barge-in', () => {
    const seen = [
      ...healthy().map((e) =>
        e.utteranceId === 'u2' && e.speaker
          ? { ...e, speaker: 'caller', speakerRechecked: true }
          : e,
      ),
      ev('barge_in', 17_200, { utteranceId: 'u2' }),
    ].sort((a, b) => a.atMs - b.atMs)
    const failures = evaluateTurns(TURNS, seen, 'p').join('\n')
    expect(failures).toMatch(/barge_in for the bystander utterance u2/)
    expect(failures).toMatch(/expected other/)
  })

  it('fails a barge 400 ms after expected', () => {
    const base = healthy().filter((e) => e.type !== 'user_speech_partial' && e.type !== 'barge_in')
    const seen = [
      ...base,
      ev('user_speech_partial', 10_600, { utteranceId: 'u1', text: 'please check' }),
      ev('barge_in', 12_000, { utteranceId: 'u1' }),
    ].sort((a, b) => a.atMs - b.atMs)
    expect(evaluateTurns(TURNS, seen, 'p').join('\n')).toMatch(/400 ms after expected .* > 300/)
  })

  it('fails a barge without a qualifying partial', () => {
    const seen = healthy().map((e) =>
      e.type === 'user_speech_partial' ? { ...e, text: 'please' } : e,
    )
    expect(evaluateTurns(TURNS, seen, 'p').join('\n')).toMatch(
      /barge_in without a qualifying partial \(>= 2 tokens\) on u1/,
    )
  })

  it('counts tokens like the product (alphanumeric tokens only)', () => {
    expect(sttPartialTokenCount('Excu')).toBe(1)
    expect(sttPartialTokenCount('Excuse me, please')).toBe(3)
    expect(sttPartialTokenCount('  - ... ok  ')).toBe(1)
    expect(sttPartialTokenCount('')).toBe(0)
  })

  it('names a missing barge-in as a harness failure when the reply ended before the qualifying partial', () => {
    const withReply = (end: number, restart?: number): Seen[] => {
      const seen = [
        ...healthy().filter((e) => e.type !== 'barge_in'),
        ev('agent_speaking_start', 9_000),
        ev('agent_speaking_end', end),
      ]
      if (restart != null) seen.push(ev('agent_speaking_start', restart))
      return seen.sort((a, b) => a.atMs - b.atMs)
    }
    // Reply drained at 11000, qualifying partial at 11400: still a failure, named as harness.
    const ended = evaluateTurns(TURNS, withReply(11_000), 'p').join('\n')
    expect(ended).toMatch(
      /harness: reply ended at 11000 ms before the qualifying partial at 11400 ms \(lengthen AGENT_PHRASE\)/,
    )
    expect(ended).not.toMatch(/no barge_in/)
    // Reply still playing at the partial: a real miss.
    const playing = evaluateTurns(TURNS, withReply(11_600), 'p').join('\n')
    expect(playing).toMatch(/no barge_in/)
    expect(playing).not.toMatch(/harness/)
    // A new reply started before the partial: a real miss.
    const restarted = evaluateTurns(TURNS, withReply(10_500, 10_800), 'p').join('\n')
    expect(restarted).toMatch(/no barge_in/)
    expect(restarted).not.toMatch(/harness/)
  })

  it('fails a caller turn without barge-in, and a misrecognized qualified phrase', () => {
    const noBarge = healthy().filter((e) => e.type !== 'barge_in')
    expect(evaluateTurns(TURNS, noBarge, 'p').join('\n')).toMatch(/no barge_in/)
    const wrong = healthy().map((e) =>
      e.text === 'please check my order' ? { ...e, text: 'banana' } : e,
    )
    expect(evaluateTurns(TURNS, wrong, 'p').join('\n')).toMatch(/similarity/)
  })

  it('does not judge the text of a long sentence', () => {
    const long: Turn = { ...caller, kind: 'long' }
    const seen = healthy().map((e) =>
      e.text === 'please check my order' ? { ...e, text: 'banana' } : e,
    )
    expect(evaluateTurns([long, bystander, bystander2, bystander3], seen, 'p')).toEqual([])
  })

  it('fails a vacuous pass where no bystander was heard', () => {
    const seen = healthy().filter((e) => !['u2', 'u3', 'u4'].includes(e.utteranceId ?? ''))
    expect(evaluateTurns(TURNS, seen, 'p').join('\n')).toMatch(/vacuous/)
  })

  const asUnknown = (reason?: string, withBarge = true): Seen[] => {
    const seen = healthy().map((e) =>
      e.type === 'user_speech_final' && e.utteranceId === 'u2'
        ? { ...e, speaker: 'unknown', speakerUnknownReason: reason }
        : e,
    )
    if (withBarge) seen.push(ev('barge_in', 16_900, { utteranceId: 'u2' }))
    return seen
  }

  it('accepts a short-unknown bystander together with its barge-in', () => {
    expect(evaluateTurns(TURNS, asUnknown('short'), 'p')).toEqual([])
  })

  it('fails a timeout-unknown bystander with a barge-in', () => {
    const out = evaluateTurns(TURNS, asUnknown('timeout'), 'p').join('\n')
    expect(out).toMatch(/barge_in for the bystander utterance u2/)
    expect(out).toMatch(/reason timeout/)
  })

  it('fails an unknown bystander without a reason', () => {
    expect(evaluateTurns(TURNS, asUnknown(undefined, false), 'p').join('\n')).toMatch(
      /final speaker unknown, expected other/,
    )
  })

  it('passes with exactly one other bystander and the rest short-unknown', () => {
    const seen = healthy().map((e) =>
      e.type === 'user_speech_final' && (e.utteranceId === 'u2' || e.utteranceId === 'u3')
        ? { ...e, speaker: 'unknown', speakerUnknownReason: 'short' }
        : e,
    )
    expect(evaluateTurns(TURNS, seen, 'p')).toEqual([])
  })

  it('fails a pass whose bystanders are all short-unknown (zero other)', () => {
    const seen = healthy().map((e) =>
      e.type === 'user_speech_final' && e.utteranceId !== 'u1'
        ? { ...e, speaker: 'unknown', speakerUnknownReason: 'short' }
        : e,
    )
    expect(evaluateTurns(TURNS, seen, 'p').join('\n')).toMatch(/vacuous/)
  })
})

/**
 * Stress run 41089340452, iteration 4 (both passes): the VAD opened in bystander turn 7's slot
 * (65 810, 1.54 s before the caller), the utterance ran into caller turn 8 (67 352), the re-check
 * heard the caller and released the barge-in at 69 104. Scene times as the harness records them.
 */
describe('merged caller utterance (VAD opened in the bystander slot)', () => {
  const t6: Turn = {
    role: 'main',
    startMs: 53_702,
    endMs: 58_500,
    text: 'I booked a table for four people',
    kind: 'long',
  }
  const t7: Turn = {
    role: 'bystander',
    startMs: 62_003,
    endMs: 64_500,
    text: 'Her garden is full of tomatoes',
    kind: 'background',
  }
  const t8: Turn = {
    role: 'main',
    startMs: 67_352,
    endMs: 69_200,
    text: 'charlie one two three',
    kind: 'qualified',
  }
  const t9: Turn = {
    role: 'bystander',
    startMs: 71_468,
    endMs: 74_000,
    text: 'A tall ladder stood beside the orchard wall',
    kind: 'background',
  }
  const SCENE = [t6, t7, t8, t9]

  /** Turn 6 healthy caller, turn 9 bystander tagged other (anti-vacuous). */
  const frame = (): Seen[] => [
    ev('user_speaking_start', 53_811, { utteranceId: 'u6' }),
    ev('user_speech_partial', 55_200, { utteranceId: 'u6', text: 'I booked' }),
    ev('barge_in', 55_343, { utteranceId: 'u6' }),
    ev('user_speech_final', 61_630, {
      utteranceId: 'u6',
      text: 'I booked a table for four people',
      speaker: 'caller',
    }),
    ev('user_speaking_start', 72_410, { utteranceId: 'u9' }),
    ev('user_speech_final', 75_000, { utteranceId: 'u9', text: 'a tall ladder', speaker: 'other' }),
  ]

  /** The utterance u7: start, partials, barge, speaking end, final (re-checked caller). */
  const u7 = (o: {
    startAt?: number
    partialAt?: number
    bargeAt?: number
    endAt?: number
    finalAt?: number
    rechecked?: boolean
    text?: string
  }): Seen[] => [
    ev('user_speaking_start', o.startAt ?? 65_810, { utteranceId: 'u7' }),
    ev('user_speech_partial', (o.partialAt ?? 67_648) - 1_000, {
      utteranceId: 'u7',
      text: 'suitcase',
    }),
    ev('user_speech_partial', o.partialAt ?? 67_648, { utteranceId: 'u7', text: 'suitcase room' }),
    ev('barge_in', o.bargeAt ?? 69_104, { utteranceId: 'u7' }),
    ev('user_speaking_end', o.endAt ?? 70_900, { utteranceId: 'u7' }),
    ev('user_speech_final', o.finalAt ?? 71_164, {
      utteranceId: 'u7',
      text: o.text ?? 'suitcase room. Charlie one two three passify',
      speaker: 'caller',
      ...(o.rechecked === false ? {} : { speakerRechecked: true }),
    }),
  ]

  const run = (extra: Seen[]): { failures: string; seen: Seen[] } => {
    const seen = [...frame(), ...extra].sort((a, b) => a.atMs - b.atMs)
    return { failures: evaluateTurns(SCENE, seen, 'p').join('\n'), seen }
  }

  it('(a) pairs the iteration-4 utterance with caller turn 8 and passes', () => {
    const { failures, seen } = run(u7({}))
    expect(failures).toBe('')
    const { starts, merged } = pairTurnUtterances(SCENE, seen)
    expect(starts.map((s) => s?.utteranceId)).toEqual(['u6', undefined, 'u7', 'u9'])
    expect([...merged]).toEqual([2])
    // Bound: start 65 810 + 2 x 1 500 = 68 810 (partial 67 648 earlier); barge 69 104 is +294.
    expect(expectedBargeAtMs(starts[2]!, seen)).toBe(68_810)
  })

  it('(b) fails a merged utterance whose barge-in came before the caller scene start', () => {
    // Start 65 000: re-check window ends 68 000, after the caller start (67 352), so it is merged;
    // a barge at 67 300 was released before the caller spoke.
    const { failures } = run(u7({ startAt: 65_000, partialAt: 65_500, bargeAt: 67_300 }))
    expect(failures).toMatch(
      /turn 2 .*barge-in at 67300 ms on merged utterance u7, not after the caller's scene start 67352 ms/,
    )
  })

  it('(c) fails a re-checked caller utterance that ends inside the bystander slot', () => {
    const { failures, seen } = run([
      ...u7({
        startAt: 62_100,
        partialAt: 63_000,
        bargeAt: 65_100,
        endAt: 65_000,
        finalAt: 65_300,
      }),
      ev('user_speaking_start', 67_450, { utteranceId: 'u8' }),
      ev('user_speech_partial', 68_300, { utteranceId: 'u8', text: 'charlie one' }),
      ev('barge_in', 68_960, { utteranceId: 'u8' }),
      ev('user_speech_final', 69_900, {
        utteranceId: 'u8',
        text: 'charlie one two three',
        speaker: 'caller',
      }),
    ])
    expect(failures).toMatch(/turn 1 .*barge_in for the bystander utterance u7/)
    expect(failures).toMatch(/turn 1 .*final speaker caller, expected other/)
    expect(failures).not.toMatch(/turn 2/)
    expect([...pairTurnUtterances(SCENE, seen).merged]).toEqual([])
  })

  it('(c2) keeps the bystander pairing when the caller turn has its own utterance', () => {
    // Spans the caller start, but the caller turn already has u8: no re-pairing, no steal.
    const { failures } = run([
      ...u7({}),
      ev('user_speaking_start', 67_500, { utteranceId: 'u8' }),
      ev('user_speech_final', 69_000, { utteranceId: 'u8', text: 'one', speaker: 'caller' }),
    ])
    expect(failures).toMatch(/turn 1 .*barge_in for the bystander utterance u7/)
  })

  it('(d) leaves a merged utterance that was not re-checked with its bystander slot', () => {
    const { failures, seen } = run(u7({ rechecked: false }))
    expect(failures).toMatch(/turn 1 .*barge_in for the bystander utterance u7/)
    expect(failures).toMatch(/turn 1 .*final speaker caller, expected other/)
    expect(failures).toMatch(/turn 2 .*caller speech start not detected/)
    expect(pairTurnUtterances(SCENE, seen).starts.map((s) => s?.utteranceId)).toEqual([
      'u6',
      'u7',
      undefined,
      'u9',
    ])
  })

  it('(e) zero gap: speech ending exactly at the caller scene start is not merged; 1 ms later is', () => {
    // Start 65 000: re-check window ends 68 000, after the caller start; bound 68 000, barge +50.
    const shape = { startAt: 65_000, partialAt: 65_500, bargeAt: 68_050, finalAt: 68_400 }
    const text = 'suitcase room charlie one two three'
    const atStart = run(u7({ ...shape, endAt: 67_352, text }))
    expect(atStart.failures).toMatch(/turn 1 .*barge_in for the bystander utterance u7/)
    expect(atStart.failures).toMatch(/turn 2 .*caller speech start not detected/)
    const after = run(u7({ ...shape, endAt: 67_353, text }))
    expect(after.failures).toBe('')
  })

  it('(f) re-check window ends before the caller starts: not merged', () => {
    // Start 64 300: re-check window ends 67 300, 52 ms before the caller (67 352) began.
    const { failures, seen } = run(
      u7({
        startAt: 64_300,
        partialAt: 65_000,
        bargeAt: 67_360,
        endAt: 68_500,
        finalAt: 68_800,
        text: 'suitcase room charlie one two three',
      }),
    )
    expect(failures).toMatch(/turn 1 .*barge_in for the bystander utterance u7/)
    expect([...pairTurnUtterances(SCENE, seen).merged]).toEqual([])
  })

  it('(g) boundary: caller start == start + 3000 is not merged; start + 2999 is', () => {
    const shape = (startAt: number) =>
      u7({
        startAt,
        partialAt: 65_000,
        bargeAt: 67_400,
        endAt: 68_500,
        finalAt: 68_800,
        text: 'suitcase room charlie one two three',
      })
    const atBound = run(shape(64_352))
    expect(atBound.failures).toMatch(/turn 1 .*barge_in for the bystander utterance u7/)
    expect([...pairTurnUtterances(SCENE, atBound.seen).merged]).toEqual([])
    const inside = run(shape(64_353))
    expect(inside.failures).toBe('')
    expect([...pairTurnUtterances(SCENE, inside.seen).merged]).toEqual([2])
  })

  it('judges only a contiguous caller-length window of a merged transcript', () => {
    expect(
      callerWindowSimilarity(
        'charlie one two three',
        'suitcase room. Charlie one two three passify',
      ),
    ).toBe(1)
    // Same words scattered through babble: the plain oracle says 1.00, the window says 0.50.
    const scattered = 'suitcase charlie room one hat two cat three'
    expect(callerWindowSimilarity('charlie one two three', scattered)).toBe(0.5)
    const { failures } = run(u7({ text: scattered }))
    expect(failures).toMatch(/turn 2 .*similarity 0\.50/)
  })
})
