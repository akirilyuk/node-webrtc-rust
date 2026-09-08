import { describe, expect, test } from 'vitest'

import { LANGUAGE_ID_LEGS } from './roundtrip-language-id.js'

describe('roundtrip-language-id', () => {
  test('defines three ISO legs with distinct phrases', () => {
    expect(LANGUAGE_ID_LEGS).toHaveLength(3)
    const langs = LANGUAGE_ID_LEGS.map((leg) => leg.lang)
    expect(new Set(langs).size).toBe(3)
    expect(langs).toEqual(['en', 'de', 'es'])
    for (const leg of LANGUAGE_ID_LEGS) {
      expect(leg.phrase.length).toBeGreaterThan(10)
      expect(leg.ttsId.length).toBeGreaterThan(0)
    }
  })
})
