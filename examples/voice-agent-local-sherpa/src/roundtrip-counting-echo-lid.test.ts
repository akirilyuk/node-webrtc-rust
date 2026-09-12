import { describe, expect, it } from 'vitest'

import {
  ECHO_SMOKE_REPLY_PREFIX,
  evaluateEchoLidInboundTranscript,
  formatEchoSmokeReply,
  transcriptHasPrefixBeforeCounting,
} from './roundtrip-counting-echo-lid-prefix.js'

describe('roundtrip-counting-echo-lid prefix matcher', () => {
  it('formatEchoSmokeReply keeps echo. punctuation', () => {
    expect(formatEchoSmokeReply('one two three')).toBe('echo. one two three')
    expect(ECHO_SMOKE_REPLY_PREFIX).toBe('echo. ')
  })

  it('transcriptHasPrefixBeforeCounting accepts non-digit prefix tokens', () => {
    expect(transcriptHasPrefixBeforeCounting('echo one two three')).toBe(true)
    expect(transcriptHasPrefixBeforeCounting('eka colon one two three')).toBe(true)
    expect(transcriptHasPrefixBeforeCounting('go call one two three')).toBe(true)
  })

  it('transcriptHasPrefixBeforeCounting rejects counting-only transcripts', () => {
    expect(transcriptHasPrefixBeforeCounting('one two three four five')).toBe(false)
    expect(transcriptHasPrefixBeforeCounting('One, two, three')).toBe(false)
    expect(transcriptHasPrefixBeforeCounting('. one two three')).toBe(false)
  })

  it('evaluateEchoLidInboundTranscript fails without prefix before counting', () => {
    const result = evaluateEchoLidInboundTranscript({
      recognized: 'one two three four five six seven eight nine ten',
      spokenCountingPhrase: 'one two three four five six seven eight nine ten',
      minNumberWords: 8,
    })
    expect(result.passed).toBe(false)
    expect(result.failures.some((f) => f.includes('missing any prefix before counting'))).toBe(true)
  })

  it('evaluateEchoLidInboundTranscript passes with echo prefix and enough digits', () => {
    const result = evaluateEchoLidInboundTranscript({
      recognized: 'echo one two three four five six seven eight nine ten',
      spokenCountingPhrase: 'one two three four five six seven eight nine ten',
      minNumberWords: 8,
    })
    expect(result.passed).toBe(true)
    expect(result.failures).toHaveLength(0)
  })
})
