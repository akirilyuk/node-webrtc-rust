import { describe, expect, it } from 'vitest'

import { evaluateTurns, type Seen, type Turn } from './roundtrip-noisy-bystander-helpers.js'

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
    ev('barge_in', 11_500),
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

  it('fails a barge-in during the bystander turn', () => {
    const seen = [...healthy(), ev('barge_in', 15_000)]
    expect(evaluateTurns([caller, bystander], seen, 'p').join('\n')).toMatch(
      /barge_in during a bystander/,
    )
  })

  it('fails a bystander final tagged caller', () => {
    const seen = healthy().map((e) =>
      e.utteranceId === 'u2' && e.speaker ? { ...e, speaker: 'caller' } : e,
    )
    expect(evaluateTurns([caller, bystander], seen, 'p').join('\n')).toMatch(/expected other/)
  })

  it('fails a barge-in later than checkSpeechMs + 150 ms', () => {
    const seen = healthy().map((e) => (e.type === 'barge_in' ? { ...e, atMs: 11_800 } : e))
    expect(evaluateTurns([caller, bystander], seen, 'p').join('\n')).toMatch(/> 1650/)
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
