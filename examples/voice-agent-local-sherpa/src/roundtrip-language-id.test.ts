import { describe, expect, test } from 'vitest'

import { LANGUAGE_ID_LEGS, LISTENER_MIN_SPEECH_MS } from './roundtrip-language-id.js'

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

  test("once-per-utterance LID window is speech ms, below the shortest leg phrase's ~2580 ms of speech", () => {
    expect(LISTENER_MIN_SPEECH_MS).toBeGreaterThanOrEqual(1500)
    expect(LISTENER_MIN_SPEECH_MS).toBeLessThanOrEqual(2400)
  })

  test('Spanish leg uses es-glados Piper with a distinctive Spanish opener', () => {
    const esLeg = LANGUAGE_ID_LEGS.find((leg) => leg.lang === 'es')
    expect(esLeg).toBeDefined()
    expect(esLeg!.ttsId).toBe('es')
    expect(esLeg!.phrase).toMatch(/español/i)
  })
})
