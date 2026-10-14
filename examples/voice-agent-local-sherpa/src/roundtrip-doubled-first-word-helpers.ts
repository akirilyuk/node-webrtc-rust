/**
 * Pure helpers for the doubled-first-word smoke (`roundtrip-doubled-first-word.ts`).
 *
 * Symptom (staging load tests): the agent replies "Okay. <4-5 number words>" and one client
 * `user_speech_final` reads "okay okay seven three two six ten" in ~2 % of turns.
 */

export type DoubledWordVariant = 'client' | 'nogate' | 'fast'

export const DOUBLED_WORD_VARIANTS: readonly DoubledWordVariant[] = ['client', 'nogate', 'fast']

export const DEFAULT_PHRASE_COUNT = 120
export const DEFAULT_PHRASE_SEED = 20_261_015
/** Silence between phrases (the load test has seconds of silence between agent replies). */
export const SILENCE_BETWEEN_PHRASES_MS = 6000
/** Finals that arrive up to this long after a phrase's silence still belong to that phrase. */
export const FINAL_TAIL_MS = 3000

const NUMBER_WORDS = [
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
] as const

/** Deterministic 32-bit PRNG (mulberry32). */
export function createRng(seed: number): () => number {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let t = state
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

function capitalize(word: string): string {
  return word.charAt(0).toUpperCase() + word.slice(1)
}

/**
 * `count` phrases shaped like the agent reply: "Okay. <4 or 5 number words>", with mixed
 * capitalisation of the first number word and an occasional comma ("Okay. three, two four eight one").
 */
export function generatePhrases(count: number, seed = DEFAULT_PHRASE_SEED): string[] {
  const rng = createRng(seed)
  const phrases: string[] = []
  for (let i = 0; i < count; i++) {
    const wordCount = rng() < 0.5 ? 4 : 5
    const words: string[] = []
    for (let w = 0; w < wordCount; w++) {
      words.push(NUMBER_WORDS[Math.floor(rng() * NUMBER_WORDS.length)]!)
    }
    if (rng() < 0.5) words[0] = capitalize(words[0]!)
    let tail = words.join(' ')
    if (rng() < 0.2) {
      const firstSpace = tail.indexOf(' ')
      tail = `${tail.slice(0, firstSpace)},${tail.slice(firstSpace)}`
    }
    phrases.push(`Okay. ${tail}`)
  }
  return phrases
}

/** Lowercased words with punctuation stripped. */
export function wordsOf(text: string): string[] {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9'\s]/g, ' ')
    .split(/\s+/)
    .filter((w) => w.length > 0)
}

/** True when the first two words of `text` are both "okay". */
export function isDoubledFirstWord(text: string): boolean {
  const words = wordsOf(text)
  return words.length >= 2 && words[0] === 'okay' && words[1] === 'okay'
}

export interface TimedFinal {
  atMs: number
  text: string
}

export interface PhraseWindow {
  index: number
  startMs: number
  /** End of the phrase's silence; finals up to `endMs + FINAL_TAIL_MS` still count. */
  endMs: number
}

/**
 * Attach each final to the phrase whose window (start .. end + tail) contains it. A final that
 * falls into no window is dropped; overlapping tails resolve to the later phrase.
 */
export function attributeFinals(
  finals: readonly TimedFinal[],
  windows: readonly PhraseWindow[],
  tailMs = FINAL_TAIL_MS,
): Map<number, TimedFinal[]> {
  const byPhrase = new Map<number, TimedFinal[]>()
  for (const final of finals) {
    let owner: PhraseWindow | undefined
    for (const w of windows) {
      if (final.atMs >= w.startMs && final.atMs < w.endMs + tailMs) owner = w
    }
    if (owner === undefined) continue
    const list = byPhrase.get(owner.index) ?? []
    list.push(final)
    byPhrase.set(owner.index, list)
  }
  return byPhrase
}

export function parseVariant(raw: string | undefined): DoubledWordVariant {
  const value = (raw ?? '').trim()
  if (value === '') return 'client'
  if ((DOUBLED_WORD_VARIANTS as readonly string[]).includes(value)) {
    return value as DoubledWordVariant
  }
  throw new Error(
    `DOUBLED_WORD_VARIANT must be one of ${DOUBLED_WORD_VARIANTS.join(', ')} (got "${value}")`,
  )
}

export function parsePhraseCount(raw: string | undefined): number {
  const value = (raw ?? '').trim()
  if (value === '') return DEFAULT_PHRASE_COUNT
  const n = Number.parseInt(value, 10)
  if (!Number.isFinite(n) || n <= 0) {
    throw new Error(`DOUBLED_WORD_PHRASES must be a positive integer (got "${value}")`)
  }
  return n
}

/** Whether the variant writes frames at 20 ms real-time pace. */
export function variantIsPaced(variant: DoubledWordVariant): boolean {
  return variant !== 'fast'
}

/** `gateStt` for the variant: only `nogate` streams audio to STT without the VAD gate. */
export function variantGateStt(variant: DoubledWordVariant): boolean {
  return variant !== 'nogate'
}

/** Wall-clock budget: render (cached or not) + feed per phrase + fixed overhead. */
export function wallClockBudgetMs(phraseCount: number, variant: DoubledWordVariant): number {
  const perPhraseMs = variantIsPaced(variant)
    ? 5000 + 4000 + SILENCE_BETWEEN_PHRASES_MS + FINAL_TAIL_MS + 2000
    : 5000 + 4000 + FINAL_TAIL_MS + 2000
  return 300_000 + phraseCount * perPhraseMs
}
