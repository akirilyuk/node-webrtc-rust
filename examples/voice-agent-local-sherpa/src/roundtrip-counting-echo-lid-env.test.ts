import { describe, expect, it } from 'vitest'

import {
  buildEchoLanguageIdConfig,
  formatSherpaLidTtsExclusionLabel,
  parseSherpaLidTtsExclusion,
  resolveMultiSessionCount,
} from './roundtrip-counting-echo-lid-env.js'

describe('parseSherpaLidTtsExclusion', () => {
  it('returns undefined when unset', () => {
    expect(parseSherpaLidTtsExclusion({})).toBeUndefined()
  })

  it('accepts on/true/1', () => {
    expect(parseSherpaLidTtsExclusion({ SHERPA_LID_TTS_EXCLUSION: '1' })).toBe(true)
    expect(parseSherpaLidTtsExclusion({ SHERPA_LID_TTS_EXCLUSION: 'true' })).toBe(true)
    expect(parseSherpaLidTtsExclusion({ SHERPA_LID_TTS_EXCLUSION: 'on' })).toBe(true)
    expect(parseSherpaLidTtsExclusion({ SHERPA_LID_TTS_EXCLUSION: 'ON' })).toBe(true)
  })

  it('accepts off/false/0', () => {
    expect(parseSherpaLidTtsExclusion({ SHERPA_LID_TTS_EXCLUSION: '0' })).toBe(false)
    expect(parseSherpaLidTtsExclusion({ SHERPA_LID_TTS_EXCLUSION: 'false' })).toBe(false)
    expect(parseSherpaLidTtsExclusion({ SHERPA_LID_TTS_EXCLUSION: 'off' })).toBe(false)
  })

  it('returns undefined for junk values', () => {
    expect(parseSherpaLidTtsExclusion({ SHERPA_LID_TTS_EXCLUSION: 'maybe' })).toBeUndefined()
    expect(parseSherpaLidTtsExclusion({ SHERPA_LID_TTS_EXCLUSION: '2' })).toBeUndefined()
  })
})

describe('formatSherpaLidTtsExclusionLabel', () => {
  it('labels unset, on, and off', () => {
    expect(formatSherpaLidTtsExclusionLabel(undefined)).toBe('library default')
    expect(formatSherpaLidTtsExclusionLabel(true)).toBe('on')
    expect(formatSherpaLidTtsExclusionLabel(false)).toBe('off')
  })
})

describe('buildEchoLanguageIdConfig', () => {
  it('omits ttsExclusion when unset', () => {
    const config = buildEchoLanguageIdConfig('/models/lid', undefined)
    expect(config.modelPath).toBe('/models/lid')
    expect(config.ttsExclusion).toBeUndefined()
  })

  it('sets ttsExclusion when provided', () => {
    expect(buildEchoLanguageIdConfig('/models/lid', true).ttsExclusion).toBe(true)
    expect(buildEchoLanguageIdConfig('/models/lid', false).ttsExclusion).toBe(false)
  })
})

describe('resolveMultiSessionCount', () => {
  it('defaults to 10', () => {
    expect(resolveMultiSessionCount({})).toBe(10)
  })

  it('prefers SHERPA_MULTI_SESSIONS over SESSIONS', () => {
    expect(resolveMultiSessionCount({ SHERPA_MULTI_SESSIONS: '3', SESSIONS: '8' })).toBe(3)
  })

  it('falls back to SESSIONS', () => {
    expect(resolveMultiSessionCount({ SESSIONS: '5' })).toBe(5)
  })

  it('clamps to min 1 and max 20', () => {
    expect(resolveMultiSessionCount({ SHERPA_MULTI_SESSIONS: '0' })).toBe(1)
    expect(resolveMultiSessionCount({ SHERPA_MULTI_SESSIONS: '99' })).toBe(20)
  })
})
