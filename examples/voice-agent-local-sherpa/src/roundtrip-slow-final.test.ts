import { describe, expect, it } from 'vitest'

import {
  AUDIBLE_RMS_THRESHOLD,
  BARGE_MAX_MS,
  FILLER_AFTER_PHRASE_END_MS,
  FILLER_TEXT,
  FINAL_DELAY_MS,
  NEXT_TURN_START_MAX_MS,
  SPEAKING_END_AFTER_BARGE_MAX_MS,
  checkAudioSilencedAfterBarge,
  checkFinalOrder,
  checkNextTurnOnTime,
  checkOverlapsPendingFinal,
  checkNoSttNotFound,
  checkRepliesStarted,
  checkSemanticBargeAfterEarlierFinal,
  checkVadBargeOnTime,
  formatTimeline,
  pcmRms,
  utteranceIdOf,
  type PhraseRef,
  type ReceivedFrame,
  type SlowFinalEvent,
} from './roundtrip-slow-final-helpers.js'

const ev = (
  type: string,
  atMs: number,
  extra: { utteranceId?: string; text?: string } = {},
): SlowFinalEvent => ({ type, atMs, ...extra })

function reason(r: { ok: boolean; reason?: string }): string {
  return r.ok ? '' : (r.reason ?? '')
}

describe('constants', () => {
  it('match the documented bounds', () => {
    expect(NEXT_TURN_START_MAX_MS).toBe(300)
    expect(BARGE_MAX_MS).toBe(300)
    expect(SPEAKING_END_AFTER_BARGE_MAX_MS).toBe(300)
    expect(FINAL_DELAY_MS).toBe(6000)
    expect(FILLER_AFTER_PHRASE_END_MS).toBe(1500)
  })

  it('FILLER_TEXT is long enough to outlast the semantic barge-in wait', () => {
    expect(FILLER_TEXT.split(/\s+/).filter((w) => w.length > 0).length).toBeGreaterThanOrEqual(30)
  })
})

describe('checkNextTurnOnTime', () => {
  const p2: PhraseRef = { label: 'P2', sentAtMs: 4000, word: 'delta' }
  const p1Start = ev('user_speaking_start', 700, { utteranceId: 'u1' })

  it('passes when P2 starts 250 ms after its first voiced frame', () => {
    const events = [p1Start, ev('user_speaking_start', 4250, { utteranceId: 'u2' })]
    expect(checkNextTurnOnTime({ events, next: p2 })).toEqual({ ok: true })
  })

  it('ignores the start of the earlier phrase', () => {
    const events = [p1Start]
    expect(checkNextTurnOnTime({ events, next: p2 }).ok).toBe(false)
  })

  it('fails on the old-code shape: user_speaking_start 6.1 s after the send', () => {
    const events = [p1Start, ev('user_speaking_start', 4000 + 6100, { utteranceId: 'u2' })]
    const r = checkNextTurnOnTime({ events, next: p2 })
    expect(r.ok).toBe(false)
    expect(reason(r)).toContain('6100 ms')
    expect(reason(r)).toContain('P2')
  })

  it('fails at 301 ms and passes at exactly 300 ms', () => {
    const at = (ms: number) => [ev('user_speaking_start', 4000 + ms, { utteranceId: 'u2' })]
    expect(checkNextTurnOnTime({ events: at(300), next: p2 }).ok).toBe(true)
    expect(checkNextTurnOnTime({ events: at(301), next: p2 }).ok).toBe(false)
  })
})

describe('checkFinalOrder', () => {
  const p1: PhraseRef = { label: 'P1', sentAtMs: 500, word: 'alpha' }
  const p2: PhraseRef = { label: 'P2', sentAtMs: 4000, word: 'delta' }
  const base = [
    ev('user_speaking_start', 700, { utteranceId: 'u1' }),
    ev('user_speaking_start', 4200, { utteranceId: 'u2' }),
  ]

  it('passes with finals in order, matching ids and words', () => {
    const events = [
      ...base,
      ev('user_speech_final', 9000, { utteranceId: 'u1', text: 'Alpha bravo charlie.' }),
      ev('user_speech_final', 12_000, { utteranceId: 'u2', text: 'delta echo foxtrot' }),
    ]
    expect(checkFinalOrder({ events, first: p1, second: p2 })).toEqual({ ok: true })
  })

  it('fails when P2 final arrives before P1 final', () => {
    const events = [
      ...base,
      ev('user_speech_final', 9000, { utteranceId: 'u2', text: 'delta echo foxtrot' }),
      ev('user_speech_final', 12_000, { utteranceId: 'u1', text: 'alpha bravo charlie' }),
    ]
    const r = checkFinalOrder({ events, first: p1, second: p2 })
    expect(r.ok).toBe(false)
    expect(reason(r)).toContain('before P1')
  })

  it('fails when a final carries the wrong utterance id', () => {
    const events = [
      ...base,
      ev('user_speech_final', 9000, { utteranceId: 'other', text: 'alpha bravo charlie' }),
      ev('user_speech_final', 12_000, { utteranceId: 'u2', text: 'delta echo foxtrot' }),
    ]
    const r = checkFinalOrder({ events, first: p1, second: p2 })
    expect(r.ok).toBe(false)
    expect(reason(r)).toContain('P1')
  })

  it('fails when the final text lacks the word', () => {
    const events = [
      ...base,
      ev('user_speech_final', 9000, { utteranceId: 'u1', text: 'alpha bravo charlie' }),
      ev('user_speech_final', 12_000, { utteranceId: 'u2', text: 'delight echo foxtrot' }),
    ]
    const r = checkFinalOrder({ events, first: p1, second: p2 })
    expect(r.ok).toBe(false)
    expect(reason(r)).toContain('"delta"')
  })

  it('skips the word check when a phrase has none', () => {
    const events = [
      ...base,
      ev('user_speech_final', 9000, { utteranceId: 'u1', text: 'whatever' }),
      ev('user_speech_final', 12_000, { utteranceId: 'u2', text: 'else' }),
    ]
    expect(
      checkFinalOrder({
        events,
        first: { label: 'P3', sentAtMs: 500 },
        second: { label: 'P4', sentAtMs: 4000 },
      }).ok,
    ).toBe(true)
  })
})

describe('checkNoSttNotFound and checkRepliesStarted', () => {
  it('flags user_stt_not_found', () => {
    expect(checkNoSttNotFound([ev('user_speech_final', 1)])).toEqual({ ok: true })
    const r = checkNoSttNotFound([ev('user_stt_not_found', 7000)])
    expect(r.ok).toBe(false)
    expect(reason(r)).toContain('+7000')
  })

  it('needs one agent_speaking_start per expected reply', () => {
    const one = [ev('agent_speaking_start', 9000)]
    expect(checkRepliesStarted({ events: one, expected: 1 }).ok).toBe(true)
    const r = checkRepliesStarted({ events: one, expected: 2 })
    expect(r.ok).toBe(false)
    expect(reason(r)).toContain('got 1')
  })
})

describe('checkVadBargeOnTime', () => {
  const p4: PhraseRef = { label: 'P4', sentAtMs: 5000 }

  it('passes with barge_in 200 ms and agent_speaking_end 100 ms later', () => {
    const events = [ev('barge_in', 5200), ev('agent_speaking_end', 5300)]
    expect(checkVadBargeOnTime({ events, interrupt: p4 })).toEqual({ ok: true })
  })

  it('fails on the old-code shape: barge_in 6.1 s late', () => {
    const events = [ev('barge_in', 5000 + 6100), ev('agent_speaking_end', 5000 + 6200)]
    const r = checkVadBargeOnTime({ events, interrupt: p4 })
    expect(r.ok).toBe(false)
    expect(reason(r)).toContain('6100 ms')
  })

  it('fails when agent_speaking_end lags barge_in by 400 ms', () => {
    const events = [ev('barge_in', 5200), ev('agent_speaking_end', 5600)]
    const r = checkVadBargeOnTime({ events, interrupt: p4 })
    expect(r.ok).toBe(false)
    expect(reason(r)).toContain('agent_speaking_end 400 ms')
  })

  it('fails without barge_in, without agent_speaking_end, or with barge_in before the send', () => {
    expect(checkVadBargeOnTime({ events: [], interrupt: p4 }).ok).toBe(false)
    expect(checkVadBargeOnTime({ events: [ev('barge_in', 5100)], interrupt: p4 }).ok).toBe(false)
    const early = checkVadBargeOnTime({
      events: [ev('barge_in', 4000), ev('agent_speaking_end', 4100)],
      interrupt: p4,
    })
    expect(early.ok).toBe(false)
    expect(reason(early)).toContain('before P4 was sent')
  })

  it('ignores an agent_speaking_end that came before barge_in', () => {
    const events = [ev('agent_speaking_end', 4000), ev('barge_in', 5200)]
    expect(reason(checkVadBargeOnTime({ events, interrupt: p4 }))).toContain(
      'no agent_speaking_end after barge_in',
    )
  })
})

describe('checkAudioSilencedAfterBarge', () => {
  const loud = AUDIBLE_RMS_THRESHOLD * 5
  const frame = (atMs: number, rms: number): ReceivedFrame => ({ atMs, rms })

  it('passes when the agent was audible, then silent from +500 ms to +1500 ms', () => {
    const frames = [frame(3000, loud), frame(5100, loud), frame(5400, loud), frame(5700, 0)]
    expect(checkAudioSilencedAfterBarge({ frames, agentStartAtMs: 3000, bargeAtMs: 5200 })).toEqual(
      { ok: true },
    )
  })

  it('tolerates the jitter-buffer tail before +500 ms', () => {
    const frames = [frame(3000, loud), frame(5600, loud), frame(5800, 0)]
    expect(checkAudioSilencedAfterBarge({ frames, agentStartAtMs: 3000, bargeAtMs: 5200 }).ok).toBe(
      true,
    )
  })

  it('fails when audible audio continues after the window start', () => {
    const frames = [frame(3000, loud), frame(5800, loud)]
    const r = checkAudioSilencedAfterBarge({ frames, agentStartAtMs: 3000, bargeAtMs: 5200 })
    expect(r.ok).toBe(false)
    expect(reason(r)).toContain('600 ms after barge_in')
  })

  it('ignores audio after the window end (the next reply)', () => {
    const frames = [frame(3000, loud), frame(7000, loud)]
    expect(checkAudioSilencedAfterBarge({ frames, agentStartAtMs: 3000, bargeAtMs: 5200 }).ok).toBe(
      true,
    )
  })

  it('fails when the meter never saw the agent speak', () => {
    const r = checkAudioSilencedAfterBarge({
      frames: [frame(3000, 0), frame(4000, 0.001)],
      agentStartAtMs: 3000,
      bargeAtMs: 5200,
    })
    expect(r.ok).toBe(false)
    expect(reason(r)).toContain('meter saw nothing')
  })
})

describe('checkSemanticBargeAfterEarlierFinal', () => {
  const p5: PhraseRef = { label: 'P5', sentAtMs: 500 }
  const p6: PhraseRef = { label: 'P6', sentAtMs: 3500, word: 'papa' }
  const good = [
    ev('user_speaking_start', 700, { utteranceId: 'u5' }),
    ev('user_speaking_start', 3700, { utteranceId: 'u6' }),
    ev('user_speech_final', 8000, { utteranceId: 'u5', text: 'mike november oscar' }),
    ev('user_speech_partial', 8100, { text: 'papa quebec' }),
    ev('barge_in', 8150),
    ev('user_speech_final', 12_000, { utteranceId: 'u6', text: 'papa quebec romeo' }),
  ]

  it('passes: partial after the earlier final, barge after the partial, within 7.5 s', () => {
    expect(checkSemanticBargeAfterEarlierFinal({ events: good, earlier: p5, later: p6 })).toEqual({
      ok: true,
    })
  })

  it('fails when the partial comes before the earlier final', () => {
    const events = [
      good[0]!,
      good[1]!,
      ev('user_speech_partial', 5000, { text: 'papa quebec' }),
      ev('user_speech_final', 8000, { utteranceId: 'u5', text: 'mike november oscar' }),
      ev('barge_in', 8100),
      good[5]!,
    ]
    const r = checkSemanticBargeAfterEarlierFinal({ events, earlier: p5, later: p6 })
    expect(r.ok).toBe(false)
    expect(reason(r)).toContain("before P5's final")
  })

  it('fails when barge_in comes before the first partial', () => {
    const events = [good[0]!, good[1]!, good[2]!, ev('barge_in', 8050), good[3]!, good[5]!]
    const r = checkSemanticBargeAfterEarlierFinal({ events, earlier: p5, later: p6 })
    expect(r.ok).toBe(false)
    expect(reason(r)).toContain('before P6')
  })

  it('fails on the late-barge shape: barge_in 7.6 s after the send', () => {
    const events = [...good.slice(0, 4), ev('barge_in', 3500 + 7600), good[5]!]
    const r = checkSemanticBargeAfterEarlierFinal({ events, earlier: p5, later: p6 })
    expect(r.ok).toBe(false)
    expect(reason(r)).toContain('7600 ms')
  })

  it('passes at exactly 7500 ms after the send', () => {
    const events = [
      ...good.slice(0, 3),
      ev('user_speech_partial', 10_900, { text: 'papa quebec' }),
      ev('barge_in', 3500 + 7500),
      good[5]!,
    ]
    expect(checkSemanticBargeAfterEarlierFinal({ events, earlier: p5, later: p6 }).ok).toBe(true)
  })

  it('fails without a barge_in, without a partial, or with a wrong P6 final', () => {
    const noBarge = good.filter((e) => e.type !== 'barge_in')
    expect(
      reason(checkSemanticBargeAfterEarlierFinal({ events: noBarge, earlier: p5, later: p6 })),
    ).toContain('no barge_in')
    const noPartial = good.filter((e) => e.type !== 'user_speech_partial')
    expect(
      reason(checkSemanticBargeAfterEarlierFinal({ events: noPartial, earlier: p5, later: p6 })),
    ).toContain('no user_speech_partial')
    const wrong = [
      ...good.slice(0, 5),
      ev('user_speech_final', 12_000, { utteranceId: 'u6', text: 'pop' }),
    ]
    expect(
      reason(checkSemanticBargeAfterEarlierFinal({ events: wrong, earlier: p5, later: p6 })),
    ).toContain('"papa"')
  })

  it('fails when the earlier phrase never got a final', () => {
    const events = good.filter((e) => !(e.type === 'user_speech_final' && e.utteranceId === 'u5'))
    expect(
      reason(checkSemanticBargeAfterEarlierFinal({ events, earlier: p5, later: p6 })),
    ).toContain('P5: no user_speech_final')
  })
})

describe('pcmRms', () => {
  it('is 0 for silence and about amplitude/32768 for a constant signal', () => {
    expect(pcmRms(Buffer.alloc(3840))).toBe(0)
    const buf = Buffer.alloc(3840)
    for (let i = 0; i < buf.length; i += 2) buf.writeInt16LE(8192, i)
    expect(pcmRms(buf)).toBeCloseTo(0.25, 5)
    expect(pcmRms(Buffer.alloc(0))).toBe(0)
  })
})

describe('formatTimeline', () => {
  it('merges events and speaker sends in time order', () => {
    const lines = formatTimeline(
      [ev('user_speech_final', 9000, { utteranceId: 'u1', text: 'alpha' })],
      [{ label: 'P1', sentAtMs: 500 }],
    )
    expect(lines).toEqual(['+500ms [speaker sent P1]', '+9000ms user_speech_final id=u1 "alpha"'])
  })
})

describe('checkOverlapsPendingFinal', () => {
  const events = [
    ev('user_speaking_start', 700, { utteranceId: 'u1' }),
    ev('user_speaking_start', 4200, { utteranceId: 'u2' }),
    ev('barge_in', 4400),
    ev('user_speech_final', 7500, { utteranceId: 'u1', text: 'alpha' }),
    ev('user_speech_final', 11_000, { utteranceId: 'u2', text: 'delta' }),
  ]

  it('passes when the later user_speaking_start precedes the earlier final', () => {
    expect(checkOverlapsPendingFinal(events, 'user_speaking_start', 'u2', 'u1')).toEqual({
      ok: true,
    })
  })

  it('passes when barge_in (no utterance id) precedes the earlier final', () => {
    expect(checkOverlapsPendingFinal(events, 'barge_in', undefined, 'u1')).toEqual({ ok: true })
  })

  it('fails when the later event came after the earlier final (final was never held back)', () => {
    const late = [
      ev('user_speaking_start', 700, { utteranceId: 'u1' }),
      ev('user_speech_final', 1800, { utteranceId: 'u1', text: 'alpha' }),
      ev('user_speaking_start', 4200, { utteranceId: 'u2' }),
    ]
    const r = checkOverlapsPendingFinal(late, 'user_speaking_start', 'u2', 'u1')
    expect(r.ok).toBe(false)
    expect(reason(r)).toContain('did not overlap the pending final; the smoke proved nothing')
  })

  it('fails at equal times, with no later event, and with no earlier final', () => {
    const tie = [
      ev('barge_in', 7500),
      ev('user_speech_final', 7500, { utteranceId: 'u1', text: 'alpha' }),
    ]
    expect(checkOverlapsPendingFinal(tie, 'barge_in', undefined, 'u1').ok).toBe(false)
    const none = checkOverlapsPendingFinal(events, 'barge_in', undefined, 'missing')
    expect(none.ok).toBe(false)
    expect(reason(none)).toContain('the smoke proved nothing')
    const noLater = checkOverlapsPendingFinal(events, 'user_speaking_start', 'nope', 'u1')
    expect(noLater.ok).toBe(false)
    expect(reason(noLater)).toContain('the smoke proved nothing')
  })
})

describe('utteranceIdOf', () => {
  it('is the id of the first user_speaking_start at or after the send', () => {
    const events = [
      ev('user_speaking_start', 700, { utteranceId: 'u1' }),
      ev('user_speaking_start', 4200, { utteranceId: 'u2' }),
    ]
    expect(utteranceIdOf(events, { label: 'P2', sentAtMs: 4000 })).toBe('u2')
    expect(utteranceIdOf(events, { label: 'P9', sentAtMs: 9000 })).toBeUndefined()
  })
})
