import { describe, expect, it } from 'vitest'

import { SPEECH_EVENT_TYPE } from '@node-webrtc-rust/sdk/voice'

import {
  assertFullCountingEcho,
  evaluateEchoLidEventOrdering,
} from './roundtrip-counting-echo-lid-multi-assert.js'

describe('assertFullCountingEcho', () => {
  const full = 'echo one two three four five six seven eight nine ten'

  it('accepts full phrase with prefix and all digits in order', () => {
    const result = assertFullCountingEcho(full)
    expect(result.ok).toBe(true)
    expect(result.failures).toHaveLength(0)
  })

  it('accepts punctuation variants (Echo: one, two … ten)', () => {
    const result = assertFullCountingEcho(
      'Echo: one, two, three, four, five, six, seven, eight, nine, ten',
    )
    expect(result.ok).toBe(true)
  })

  it('fails when prefix before counting is missing', () => {
    const result = assertFullCountingEcho('one two three four five six seven eight nine ten')
    expect(result.ok).toBe(false)
    expect(result.failures.some((f) => f.includes('prefix'))).toBe(true)
  })

  it('fails when the first counting word is missing', () => {
    const result = assertFullCountingEcho('echo two three four five six seven eight nine ten')
    expect(result.ok).toBe(false)
    expect(result.failures.some((f) => f.includes('"one"'))).toBe(true)
  })

  it('fails when a middle counting word is missing', () => {
    const result = assertFullCountingEcho('echo one two three four six seven eight nine ten')
    expect(result.ok).toBe(false)
    expect(result.failures.some((f) => f.includes('"five"'))).toBe(true)
  })

  it('fails when the last counting word is missing', () => {
    const result = assertFullCountingEcho('echo one two three four five six seven eight nine')
    expect(result.ok).toBe(false)
    expect(result.failures.some((f) => f.includes('"ten"'))).toBe(true)
  })

  it('fails when digits are out of order', () => {
    const result = assertFullCountingEcho('echo one two three four five six eight seven nine ten')
    expect(result.ok).toBe(false)
    expect(result.failures.some((f) => f.includes('expected "seven"'))).toBe(true)
  })
})

describe('evaluateEchoLidEventOrdering', () => {
  it('passes when user_language precedes close sequence and agent TTS', () => {
    const events = [
      { type: SPEECH_EVENT_TYPE.sttStreamEnd, atMs: 1 },
      { type: SPEECH_EVENT_TYPE.userSttEnd, atMs: 2 },
      { type: SPEECH_EVENT_TYPE.userLanguage, atMs: 3 },
      { type: SPEECH_EVENT_TYPE.userSpeakingEnd, atMs: 4 },
      { type: SPEECH_EVENT_TYPE.userSpeechFinal, atMs: 5, text: 'one two' },
      { type: SPEECH_EVENT_TYPE.agentSpeakingStart, atMs: 6 },
    ]
    expect(evaluateEchoLidEventOrdering({ events }).passed).toBe(true)
  })

  it('fails when user_language follows user_speech_final', () => {
    const events = [
      { type: SPEECH_EVENT_TYPE.userSpeakingEnd, atMs: 1 },
      { type: SPEECH_EVENT_TYPE.userSpeechFinal, atMs: 2 },
      { type: SPEECH_EVENT_TYPE.userLanguage, atMs: 3 },
    ]
    const result = evaluateEchoLidEventOrdering({ events })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('user_language'))).toBe(true)
  })
})
