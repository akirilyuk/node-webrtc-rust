import { describe, expect, test } from 'vitest'

import { languageIdAllowlistMaxGap } from '../src/voice/VoiceAgent'

describe('languageId.allowlistMaxGap', () => {
  test('undefined keeps the native default', () => {
    expect(languageIdAllowlistMaxGap(undefined)).toBeUndefined()
  })

  test('finite values >= 0 pass through', () => {
    expect(languageIdAllowlistMaxGap(0)).toBe(0)
    expect(languageIdAllowlistMaxGap(3)).toBe(3)
    expect(languageIdAllowlistMaxGap(2.5)).toBe(2.5)
  })

  test('negative, NaN, Infinity and non-numbers are rejected', () => {
    for (const bad of [-0.5, Number.NaN, Number.POSITIVE_INFINITY, '3' as unknown as number]) {
      expect(() => languageIdAllowlistMaxGap(bad)).toThrow(TypeError)
    }
  })
})
