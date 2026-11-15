import { describe, expect, test } from 'vitest'

import { languageIdAllowlistMinMargin } from '../src/voice/VoiceAgent'

describe('languageId.allowlistMinMargin', () => {
  test('undefined keeps the native default', () => {
    expect(languageIdAllowlistMinMargin(undefined)).toBeUndefined()
  })

  test('finite values >= 0 pass through', () => {
    expect(languageIdAllowlistMinMargin(0)).toBe(0)
    expect(languageIdAllowlistMinMargin(2)).toBe(2)
    expect(languageIdAllowlistMinMargin(1.5)).toBe(1.5)
  })

  test('negative, NaN, Infinity and non-numbers are rejected', () => {
    for (const bad of [-0.5, Number.NaN, Number.POSITIVE_INFINITY, '2' as unknown as number]) {
      expect(() => languageIdAllowlistMinMargin(bad)).toThrow(TypeError)
    }
  })
})
