import { describe, expect, it } from 'vitest'

import {
  DEFAULT_COUNTING_PHRASE_ONE_TO_TEN,
  NUMBER_WORDS_ONE_TO_TEN,
} from './roundtrip-counting.js'
import {
  DEFAULT_LONG_SENTENCE_PHRASE,
  ECHO_REPLY_PREFIX,
  echoNumberWordRetention,
  evaluateCountingEchoLeg,
  evaluateEchoRound,
  evaluateSentenceEchoLeg,
  formatAgent2EchoReply,
  transcriptIncludesYouSaid,
} from './roundtrip-counting-echo.js'

const okStats = {
  finals: ['one two three four five six seven eight nine ten'],
  speakingEndCount: 1,
  speakingStartCount: 1,
  partialCount: 3,
  bargeInCount: 0,
  speakingEndAtMs: 1000,
  speechFinalAtMs: 1100,
}

describe('roundtrip-counting-echo helpers', () => {
  it('formatAgent2EchoReply prefixes You said:', () => {
    expect(formatAgent2EchoReply('hello world')).toBe('You said: hello world')
    expect(formatAgent2EchoReply('')).toBe('You said:')
  })

  it('DEFAULT_LONG_SENTENCE_PHRASE is non-empty', () => {
    expect(DEFAULT_LONG_SENTENCE_PHRASE.length).toBeGreaterThan(80)
    expect(DEFAULT_LONG_SENTENCE_PHRASE).toContain('lenght')
  })

  it('echoNumberWordRetention compares number tokens across legs', () => {
    const a = 'one two three four five six seven eight nine ten'
    const b = 'you said one two three four five six seven eight nine ten'
    expect(echoNumberWordRetention(a, b)).toBe(1)
    expect(echoNumberWordRetention(a, 'one two three')).toBeCloseTo(0.3, 1)
  })

  it('transcriptIncludesYouSaid detects echo prefix', () => {
    expect(transcriptIncludesYouSaid('You said: hello')).toBe(true)
    expect(transcriptIncludesYouSaid('He uses one two three')).toBe(true)
    expect(transcriptIncludesYouSaid('hello only')).toBe(false)
  })

  it('evaluateEchoRound passes counting round when legs ok', () => {
    const text = 'one two three four five six seven eight nine ten'
    const legA = evaluateCountingEchoLeg({
      phrase: DEFAULT_COUNTING_PHRASE_ONE_TO_TEN,
      recognized: text,
      stats: okStats,
      label: 'A',
      minNumberWords: 8,
    })
    const echoText = formatAgent2EchoReply(text)
    const legB = evaluateCountingEchoLeg({
      phrase: echoText,
      recognized: `you said ${text}`,
      stats: okStats,
      label: 'B',
      minNumberWords: 8,
      requireYouSaid: true,
    })
    const result = evaluateEchoRound({
      name: 'counting',
      kind: 'counting',
      sourcePhrase: DEFAULT_COUNTING_PHRASE_ONE_TO_TEN,
      legA,
      legB,
    })
    expect(result.passed).toBe(true)
    expect(ECHO_REPLY_PREFIX).toBe('You said: ')
    expect(NUMBER_WORDS_ONE_TO_TEN).toHaveLength(10)
  })

  it('evaluateSentenceEchoLeg checks similarity and you said on echo leg', () => {
    const source = DEFAULT_LONG_SENTENCE_PHRASE
    const heard = source.replace('lenght', 'length')
    const legA = evaluateSentenceEchoLeg({
      phrase: source,
      recognized: heard,
      stats: okStats,
      label: 'A',
      minSimilarity: 0.85,
    })
    expect(legA.passed).toBe(true)

    const echoPhrase = formatAgent2EchoReply(heard)
    const legB = evaluateSentenceEchoLeg({
      phrase: echoPhrase,
      recognized: `you said ${heard}`,
      stats: okStats,
      label: 'B',
      minSimilarity: 0.5,
      requireYouSaid: true,
    })
    expect(legB.passed).toBe(true)
  })
})
