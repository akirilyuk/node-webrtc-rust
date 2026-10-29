import { describe, expect, it } from 'vitest'

import {
  evaluateTurns,
  expectedBargeAtMs,
  matchTurnUtterances,
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
  ]
}

describe('evaluateTurns', () => {
  it('passes a healthy pass', () => {
    expect(evaluateTurns([caller, bystander], healthy(), 'p')).toEqual([])
  })

  it('fails a barge-in that carries the bystander utterance id', () => {
    const seen = [...healthy(), ev('barge_in', 15_000, { utteranceId: 'u2' })]
    expect(evaluateTurns([caller, bystander], seen, 'p').join('\n')).toMatch(
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
    expect(evaluateTurns([caller, bystander], seen, 'p').join('\n')).toMatch(
      /u3 whose final is speaker other/,
    )
  })

  it('attributes a late barge of the previous caller utterance to that utterance', () => {
    // Inside the bystander's time window, but the id says it is the caller's utterance u1.
    const seen = [...healthy(), ev('barge_in', 14_500, { utteranceId: 'u1' })]
    seen.sort((a, b) => a.atMs - b.atMs)
    expect(evaluateTurns([caller, bystander], seen, 'p')).toEqual([])
  })

  it('fails a barge_in without utteranceId', () => {
    const seen = [...healthy(), ev('barge_in', 12_500)]
    expect(evaluateTurns([caller, bystander], seen, 'p').join('\n')).toMatch(/without utteranceId/)
  })

  it('matches turns to utterances by the first speaking start inside the turn slot', () => {
    // u2 starts 2.3 s after the bystander's scene start (pipeline delay), still before any next turn.
    const seen = healthy().map((e) =>
      e.type === 'user_speaking_start' && e.utteranceId === 'u2' ? { ...e, atMs: 16_300 } : e,
    )
    expect(matchTurnUtterances([caller, bystander], seen).map((s) => s?.utteranceId)).toEqual([
      'u1',
      'u2',
    ])
    // VAD opened in the babble 700 ms before the bystander voice (caller ended at 12 000, slot
    // boundary 13 000): still the bystander's utterance.
    const early = healthy().map((e) =>
      e.type === 'user_speaking_start' && e.utteranceId === 'u2' ? { ...e, atMs: 13_300 } : e,
    )
    expect(matchTurnUtterances([caller, bystander], early).map((s) => s?.utteranceId)).toEqual([
      'u1',
      'u2',
    ])
    // A bystander merged into the caller utterance has no own start: nothing matched, no steal.
    const merged = healthy().filter((e) => e.utteranceId !== 'u2')
    expect(matchTurnUtterances([caller, bystander], merged).map((s) => s?.utteranceId)).toEqual([
      'u1',
      undefined,
    ])
  })

  it('fails a bystander final tagged caller', () => {
    const seen = healthy().map((e) =>
      e.utteranceId === 'u2' && e.speaker ? { ...e, speaker: 'caller' } : e,
    )
    expect(evaluateTurns([caller, bystander], seen, 'p').join('\n')).toMatch(/expected other/)
  })

  it('bounds the barge by the verdict when a qualifying partial came first', () => {
    // Start 10 100, qualifying partial 10 600 -> expected = verdict 11 600.
    const base = healthy().filter((e) => e.type !== 'user_speech_partial' && e.type !== 'barge_in')
    const seen = [
      ...base,
      ev('user_speech_partial', 10_600, { utteranceId: 'u1', text: 'please check' }),
      ev('barge_in', 11_650, { utteranceId: 'u1' }),
    ].sort((a, b) => a.atMs - b.atMs)
    expect(evaluateTurns([caller, bystander], seen, 'p')).toEqual([])
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
    expect(evaluateTurns([caller, bystander], seen, 'p')).toEqual([])
  })

  it('fails a barge 400 ms after expected', () => {
    const base = healthy().filter((e) => e.type !== 'user_speech_partial' && e.type !== 'barge_in')
    const seen = [
      ...base,
      ev('user_speech_partial', 10_600, { utteranceId: 'u1', text: 'please check' }),
      ev('barge_in', 12_000, { utteranceId: 'u1' }),
    ].sort((a, b) => a.atMs - b.atMs)
    expect(evaluateTurns([caller, bystander], seen, 'p').join('\n')).toMatch(
      /400 ms after expected .* > 300/,
    )
  })

  it('fails a barge without a qualifying partial', () => {
    const seen = healthy().map((e) =>
      e.type === 'user_speech_partial' ? { ...e, text: 'please' } : e,
    )
    expect(evaluateTurns([caller, bystander], seen, 'p').join('\n')).toMatch(
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
    const ended = evaluateTurns([caller, bystander], withReply(11_000), 'p').join('\n')
    expect(ended).toMatch(
      /harness: reply ended at 11000 ms before the qualifying partial at 11400 ms \(lengthen AGENT_PHRASE\)/,
    )
    expect(ended).not.toMatch(/no barge_in/)
    // Reply still playing at the partial: a real miss.
    const playing = evaluateTurns([caller, bystander], withReply(11_600), 'p').join('\n')
    expect(playing).toMatch(/no barge_in/)
    expect(playing).not.toMatch(/harness/)
    // A new reply started before the partial: a real miss.
    const restarted = evaluateTurns([caller, bystander], withReply(10_500, 10_800), 'p').join('\n')
    expect(restarted).toMatch(/no barge_in/)
    expect(restarted).not.toMatch(/harness/)
  })

  it('fails a caller turn without barge-in, and a misrecognized qualified phrase', () => {
    const noBarge = healthy().filter((e) => e.type !== 'barge_in')
    expect(evaluateTurns([caller, bystander], noBarge, 'p').join('\n')).toMatch(/no barge_in/)
    const wrong = healthy().map((e) =>
      e.text === 'please check my order' ? { ...e, text: 'banana' } : e,
    )
    expect(evaluateTurns([caller, bystander], wrong, 'p').join('\n')).toMatch(/similarity/)
  })

  it('does not judge the text of a long sentence', () => {
    const long: Turn = { ...caller, kind: 'long' }
    const seen = healthy().map((e) =>
      e.text === 'please check my order' ? { ...e, text: 'banana' } : e,
    )
    expect(evaluateTurns([long, bystander], seen, 'p')).toEqual([])
  })

  it('fails a vacuous pass where no bystander was heard', () => {
    const seen = healthy().filter((e) => e.utteranceId !== 'u2')
    expect(evaluateTurns([caller, bystander], seen, 'p').join('\n')).toMatch(/vacuous/)
  })
})
