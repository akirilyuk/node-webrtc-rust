import { describe, expect, it } from 'vitest'

import {
  attributeFinals,
  generatePhrases,
  isDoubledFirstWord,
  parsePhraseCount,
  parseVariant,
  variantGateStt,
  variantIsPaced,
  wordsOf,
  type PhraseWindow,
} from './roundtrip-doubled-first-word-helpers.js'

describe('generatePhrases', () => {
  it('is deterministic for a seed and differs across seeds', () => {
    expect(generatePhrases(50, 7)).toEqual(generatePhrases(50, 7))
    expect(generatePhrases(50, 7)).not.toEqual(generatePhrases(50, 8))
  })

  it('shapes every phrase as "Okay." plus 4 or 5 number words', () => {
    const phrases = generatePhrases(200)
    expect(phrases).toHaveLength(200)
    const numbers = new Set([
      'one',
      'two',
      'three',
      'four',
      'five',
      'six',
      'seven',
      'eight',
      'nine',
      'ten',
    ])
    for (const phrase of phrases) {
      expect(phrase.startsWith('Okay. ')).toBe(true)
      const words = wordsOf(phrase)
      expect(words[0]).toBe('okay')
      const rest = words.slice(1)
      expect([4, 5]).toContain(rest.length)
      for (const w of rest) expect(numbers.has(w)).toBe(true)
    }
  })

  it('mixes capitalisation and commas like the agent replies', () => {
    const phrases = generatePhrases(200)
    expect(phrases.some((p) => /^Okay\. [A-Z]/.test(p))).toBe(true)
    expect(phrases.some((p) => /^Okay\. [a-z]/.test(p))).toBe(true)
    expect(phrases.some((p) => p.includes(','))).toBe(true)
  })
})

describe('isDoubledFirstWord', () => {
  it('detects a doubled okay', () => {
    expect(isDoubledFirstWord('okay okay seven')).toBe(true)
    expect(isDoubledFirstWord('Okay okay, three')).toBe(true)
    expect(isDoubledFirstWord('Okay. Okay. three two')).toBe(true)
  })

  it('does not flag a single okay or other repeated words', () => {
    expect(isDoubledFirstWord('okay seven')).toBe(false)
    expect(isDoubledFirstWord('okay')).toBe(false)
    expect(isDoubledFirstWord('seven seven three')).toBe(false)
    expect(isDoubledFirstWord('seven okay okay')).toBe(false)
    expect(isDoubledFirstWord('')).toBe(false)
  })
})

describe('attributeFinals', () => {
  const windows: PhraseWindow[] = [
    { index: 0, startMs: 0, endMs: 9000 },
    { index: 1, startMs: 9000, endMs: 18_000 },
  ]

  it('assigns finals to the phrase window containing them, tail included', () => {
    const map = attributeFinals(
      [
        { atMs: 4000, text: 'a' },
        { atMs: 10_000, text: 'b' },
        { atMs: 20_500, text: 'c' },
      ],
      windows,
      3000,
    )
    expect(map.get(0)?.map((f) => f.text)).toEqual(['a'])
    expect(map.get(1)?.map((f) => f.text)).toEqual(['b', 'c'])
  })

  it('drops finals outside every window', () => {
    const map = attributeFinals([{ atMs: 30_000, text: 'late' }], windows, 3000)
    expect(map.size).toBe(0)
  })
})

describe('variants', () => {
  it('parses the variant env and rejects unknown values', () => {
    expect(parseVariant(undefined)).toBe('client')
    expect(parseVariant('')).toBe('client')
    expect(parseVariant('nogate')).toBe('nogate')
    expect(parseVariant('fast')).toBe('fast')
    expect(() => parseVariant('bogus')).toThrow(/DOUBLED_WORD_VARIANT/)
  })

  it('only nogate disables the gate and only fast skips pacing', () => {
    expect(variantGateStt('client')).toBe(true)
    expect(variantGateStt('nogate')).toBe(false)
    expect(variantGateStt('fast')).toBe(true)
    expect(variantIsPaced('client')).toBe(true)
    expect(variantIsPaced('nogate')).toBe(true)
    expect(variantIsPaced('fast')).toBe(false)
  })

  it('parses the phrase count with a default of 120', () => {
    expect(parsePhraseCount(undefined)).toBe(120)
    expect(parsePhraseCount('300')).toBe(300)
    expect(() => parsePhraseCount('0')).toThrow(/DOUBLED_WORD_PHRASES/)
  })
})
