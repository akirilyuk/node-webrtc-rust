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

  test('Spanish phrase is long enough for stable Whisper LID', () => {
    const esLeg = LANGUAGE_ID_LEGS.find((leg) => leg.lang === 'es')
    expect(esLeg).toBeDefined()
    expect(esLeg!.phrase.length).toBeGreaterThan(40)
    expect(esLeg!.ttsId).toBe('es')
  })
})
