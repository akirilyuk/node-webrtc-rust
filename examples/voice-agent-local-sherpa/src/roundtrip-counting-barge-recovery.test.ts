import {
  SEMANTIC_BARGE_IN_WORST_CASE_LATENCY_MS,
  SPEECH_EVENT_TYPE,
} from '@node-webrtc-rust/sdk/voice'
import { describe, expect, it } from 'vitest'

import { DEFAULT_COUNTING_PHRASE_ONE_TO_TEN } from './roundtrip-counting.js'
import { echoPayloadForCompare, formatAgent2EchoReply } from './roundtrip-counting-echo.js'
import { evaluateBargeUtteranceFinal } from './roundtrip-barge-in-helpers.js'
import {
  agentSpeakingSpanMs,
  DEFAULT_BARGE_PHRASE,
  DEFAULT_RECOVERY_PHRASE,
  deriveInterruptedLegCaps,
  evaluateBargeLatency,
  evaluateInterruptedEchoLeg,
} from './roundtrip-counting-barge-recovery.js'

describe('roundtrip-counting-barge-recovery helpers', () => {
  it('DEFAULT_RECOVERY_PHRASE is non-empty', () => {
    expect(DEFAULT_RECOVERY_PHRASE.length).toBeGreaterThan(10)
  })

  it('DEFAULT_BARGE_PHRASE matches semantic barge-in default', () => {
    expect(DEFAULT_BARGE_PHRASE).toBe('excuse me please')
  })

  it('echoPayloadForCompare strips You said prefix', () => {
    expect(echoPayloadForCompare('You said: one two three')).toBe('one two three')
    expect(echoPayloadForCompare('You said one two three')).toBe('one two three')
  })

  it('evaluateInterruptedEchoLeg passes CI-like partial echo after 2-token barge (~60% sim)', () => {
    const echoText = formatAgent2EchoReply('one two three four five six seven eight nine ten')
    const result = evaluateInterruptedEchoLeg({
      echoText,
      recognized: 'said one two three four five seven',
      maxNumberWords: 7,
      maxSimilarity: 0.7,
    })
    expect(result.passed).toBe(true)
    expect(result.similarity).toBe(0.6)
    expect(result.numberWordsFound).toBe(6)
  })

  it('evaluateInterruptedEchoLeg fails when too much of the echo is heard', () => {
    const echoText = formatAgent2EchoReply('one two three four five six seven eight nine ten')
    const result = evaluateInterruptedEchoLeg({
      echoText,
      recognized: 'you said one two three four five six seven eight',
      maxNumberWords: 7,
      maxSimilarity: 0.7,
    })
    expect(result.passed).toBe(false)
    expect(result.similarity).toBeGreaterThan(0.7)
  })

  it('evaluateInterruptedEchoLeg passes when transcript is a short tail', () => {
    const echoText = formatAgent2EchoReply('one two three four five six seven eight nine ten')
    const result = evaluateInterruptedEchoLeg({
      echoText,
      recognized: 'you said one two three four',
      maxNumberWords: 7,
      maxSimilarity: 0.7,
    })
    expect(result.passed).toBe(true)
    expect(result.numberWordsFound).toBeLessThanOrEqual(7)
  })

  it('evaluateInterruptedEchoLeg fails when full echo is heard', () => {
    const heard = 'one two three four five six seven eight nine ten'
    const echoText = formatAgent2EchoReply(heard)
    const result = evaluateInterruptedEchoLeg({
      echoText,
      recognized: `you said ${heard}`,
      maxNumberWords: 7,
      maxSimilarity: 0.7,
    })
    expect(result.passed).toBe(false)
    expect(result.failures.length).toBeGreaterThan(0)
  })

  it('evaluateInterruptedEchoLeg fails on empty transcript', () => {
    const echoText = formatAgent2EchoReply(DEFAULT_COUNTING_PHRASE_ONE_TO_TEN)
    const result = evaluateInterruptedEchoLeg({
      echoText,
      recognized: '',
      maxNumberWords: 7,
      maxSimilarity: 0.7,
    })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('empty'))).toBe(true)
  })

  it('Round 2 barge phrase check passes when Agent2 final matches injected phrase', () => {
    const result = evaluateBargeUtteranceFinal({
      events: [
        { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 100 },
        { type: SPEECH_EVENT_TYPE.userSpeechPartial, atMs: 900, text: 'stop now' },
        { type: SPEECH_EVENT_TYPE.bargeIn, atMs: 950 },
        { type: SPEECH_EVENT_TYPE.agentSpeakingEnd, atMs: 1000 },
        { type: SPEECH_EVENT_TYPE.userSpeakingEnd, atMs: 2200 },
        { type: SPEECH_EVENT_TYPE.userSpeechFinal, atMs: 2300, text: DEFAULT_BARGE_PHRASE },
      ],
      expectedPhrase: DEFAULT_BARGE_PHRASE,
      label: 'Round 2 barge',
    })
    expect(result.passed).toBe(true)
  })
})

/**
 * Offset sweep over the STT partial grid (`test.wrong-oracle`, nwr CI run 41462309917).
 *
 * Model, calibrated on the two CI passes of that run: partials arrive every 1280 ms
 * (Rust `SHERPA_STREAMING_PARTIAL_INTERVAL_MS`) at a phase set by scheduling; a partial holds two
 * tokens of "excuse me please" only from 1400 ms after onset (CI: 1346 ms → "Excuse",
 * 1437 ms → "Excuse me, ple"). Barge fires on the first grid partial at or after that. agent2 speaks
 * the 12-word echo in 4064 ms (round 1 of the run); the barge onset comes 520 ms after agent2
 * starts. Every word agent2 started before the flush is heard.
 */
describe('interrupted leg B oracle — partial-grid offset sweep', () => {
  const PARTIAL_INTERVAL_MS = 1280
  const TWO_TOKENS_FROM_MS = 1400
  const FULL_ECHO_WORDS = 12
  const FULL_ECHO_MS = 4064
  const ONSET_MS = 520
  const PREFIX_WORDS = 2
  const NUMBERS = ['one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight', 'nine', 'ten']
  const echoText = formatAgent2EchoReply(NUMBERS.join(' '))

  function heardAtPhase(phaseMs: number): { latencyMs: number; recognized: string } {
    // First grid point (phase + k × interval) at or after TWO_TOKENS_FROM_MS.
    const k = Math.max(0, Math.ceil((TWO_TOKENS_FROM_MS - phaseMs) / PARTIAL_INTERVAL_MS))
    const latencyMs = phaseMs + k * PARTIAL_INTERVAL_MS
    const wordsHeard = Math.ceil((FULL_ECHO_WORDS / FULL_ECHO_MS) * (ONSET_MS + latencyMs))
    const numbers = Math.min(10, Math.max(0, wordsHeard - PREFIX_WORDS))
    return { latencyMs, recognized: `You said, ${NUMBERS.slice(0, numbers).join(', ')}` }
  }

  const phases = Array.from({ length: PARTIAL_INTERVAL_MS / 20 }, (_, i) => i * 20)
  const caps = deriveInterruptedLegCaps({
    fullEchoWords: FULL_ECHO_WORDS,
    fullEchoDurationMs: FULL_ECHO_MS,
    bargeOnsetAfterSpeakStartMs: ONSET_MS,
    prefixWords: PREFIX_WORDS,
  })

  it('modelled latency never exceeds the product bound at any phase', () => {
    for (const phase of phases) {
      expect(heardAtPhase(phase).latencyMs).toBeLessThanOrEqual(
        SEMANTIC_BARGE_IN_WORST_CASE_LATENCY_MS,
      )
    }
  })

  it('the old fixed caps (7 numbers / 0.7) fail at the bad phases', () => {
    const failing = phases.filter(
      (phase) =>
        !evaluateInterruptedEchoLeg({
          echoText,
          recognized: heardAtPhase(phase).recognized,
          maxNumberWords: 7,
          maxSimilarity: 0.7,
        }).passed,
    )
    expect(failing.length).toBeGreaterThan(0)
    expect(failing.length).toBeLessThan(phases.length)
  })

  it('the derived caps pass at every phase and still prove truncation', () => {
    expect(caps.provesTruncation).toBe(true)
    for (const phase of phases) {
      const result = evaluateInterruptedEchoLeg({
        echoText,
        recognized: heardAtPhase(phase).recognized,
        maxNumberWords: caps.maxNumberWords,
        maxSimilarity: caps.maxSimilarity,
      })
      expect(result.passed, `phase ${phase}: ${result.failures.join('; ')}`).toBe(true)
    }
  })

  it('derived caps pass the CI pass 1 transcript (8 numbers, 80%) the old caps failed', () => {
    const recognized = 'You said, one, two, three, four, five, six, seven, eight'
    expect(
      evaluateInterruptedEchoLeg({ echoText, recognized, maxNumberWords: 7, maxSimilarity: 0.7 })
        .passed,
    ).toBe(false)
    expect(
      evaluateInterruptedEchoLeg({
        echoText,
        recognized,
        maxNumberWords: caps.maxNumberWords,
        maxSimilarity: caps.maxSimilarity,
      }).passed,
    ).toBe(true)
  })

  it('derived caps still fail the full echo (barge never truncated playback)', () => {
    const result = evaluateInterruptedEchoLeg({
      echoText,
      recognized: 'You said, one, two, three, four, five, six, seven, eight, nine, ten',
      maxNumberWords: caps.maxNumberWords,
      maxSimilarity: caps.maxSimilarity,
    })
    expect(result.passed).toBe(false)
  })

  it('caps report no proof of truncation when the window covers the whole echo', () => {
    const wide = deriveInterruptedLegCaps({
      fullEchoWords: FULL_ECHO_WORDS,
      fullEchoDurationMs: FULL_ECHO_MS,
      bargeOnsetAfterSpeakStartMs: 3000,
      prefixWords: PREFIX_WORDS,
    })
    expect(wide.provesTruncation).toBe(false)
  })
})

describe('evaluateBargeLatency', () => {
  const ev = (type: string, atMs: number) => ({ type, atMs }) as never

  it('passes the CI pass 1 timeline with flush end (2624 ms, worst grid phase)', () => {
    const r = evaluateBargeLatency({
      fullEchoDurationMs: 4064,
      agent1Events: [ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 30628)],
      agent2Events: [
        ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 30108),
        ev(SPEECH_EVENT_TYPE.bargeIn, 33252),
        ev(SPEECH_EVENT_TYPE.agentSpeakingEnd, 33253),
      ],
    })
    expect(r.failures).toEqual([])
    expect(r.onsetAfterSpeakStartMs).toBe(520)
    expect(r.latencyMs).toBe(2624)
  })

  it('fails when barge_in fires later than the bound', () => {
    const r = evaluateBargeLatency({
      fullEchoDurationMs: 4064,
      agent1Events: [ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 1000)],
      agent2Events: [
        ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 500),
        ev(SPEECH_EVENT_TYPE.bargeIn, 1000 + SEMANTIC_BARGE_IN_WORST_CASE_LATENCY_MS + 1),
      ],
    })
    expect(r.failures.join(' ')).toMatch(/SEMANTIC_BARGE_IN_WORST_CASE_LATENCY_MS/)
  })

  it('fails when barge_in never fires', () => {
    const r = evaluateBargeLatency({
      fullEchoDurationMs: 4064,
      agent1Events: [ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 1000)],
      agent2Events: [ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 500)],
    })
    expect(r.failures).toContain('interrupted leg B: barge_in never fired')
  })

  it('fails when agent2 keeps speaking after barge_in', () => {
    const r = evaluateBargeLatency({
      fullEchoDurationMs: 4064,
      agent1Events: [ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 500)],
      agent2Events: [
        ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 0),
        ev(SPEECH_EVENT_TYPE.bargeIn, 2000),
        ev(SPEECH_EVENT_TYPE.agentSpeakingEnd, 4064),
      ],
    })
    expect(r.failures.join(' ')).toMatch(/playback not truncated/)
  })

  it('fails when no agent_speaking_end follows barge_in', () => {
    const r = evaluateBargeLatency({
      fullEchoDurationMs: 4064,
      agent1Events: [ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 500)],
      agent2Events: [
        ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 0),
        ev(SPEECH_EVENT_TYPE.bargeIn, 2000),
      ],
    })
    expect(r.failures.join(' ')).toMatch(/playback not truncated/)
  })

  it('onset 700 ms with prompt flush passes', () => {
    const r = evaluateBargeLatency({
      fullEchoDurationMs: 4064,
      agent1Events: [ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 700)],
      agent2Events: [
        ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 0),
        ev(SPEECH_EVENT_TYPE.bargeIn, 3300),
        ev(SPEECH_EVENT_TYPE.agentSpeakingEnd, 3301),
      ],
    })
    expect(r.failures).toEqual([])
  })

  it('agentSpeakingSpanMs measures start → end', () => {
    expect(
      agentSpeakingSpanMs([
        ev(SPEECH_EVENT_TYPE.agentSpeakingStart, 14334),
        ev(SPEECH_EVENT_TYPE.agentSpeakingEnd, 18398),
      ]),
    ).toBe(4064)
  })
})
