/**
 * Prefix matcher for counting echo + LID roundtrip — mirrors e2e echo-transcript-match
 * `transcriptHasAnyPrefixBeforeCounting` / `local STT missing any prefix before counting`.
 */

export const NUMBER_WORDS_ONE_TO_TEN = [
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

function normalizeForCompare(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9\s]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}

function countNumberWordsInTranscript(
  text: string,
  numberWords: readonly string[] = NUMBER_WORDS_ONE_TO_TEN,
): number {
  const haystack = ` ${normalizeForCompare(text)} `
  let hits = 0
  for (const word of numberWords) {
    if (haystack.includes(` ${word} `)) {
      hits += 1
    }
  }
  return hits
}

/** Cloud voice-advanced default `languageId.minSpeechMs` (staging echo-smoke). */
export const CLOUD_DEFAULT_LID_MIN_SPEECH_MS = 2500

/**
 * Staging echo-smoke agent template reply prefix (keep punctuation + trailing space).
 *
 * Must be a word Kroko Zipformer reliably emits as an STT token when Piper TTS speaks it.
 * Measured offline (20 fresh Piper amy-low renders → harness VAD + Kroko): `echo.` produced
 * no leading token ~5% of the time (Evo/Edel/It/… or silence) even with 2s warm lead-in and
 * intact ~340ms audio; `Okay.` (and Hello./Ready.) had 0/N misses. The STT-level
 * `transcriptHasPrefixBeforeCounting` assertion is unchanged — it still catches real first-word
 * drops. The `@voicethere/agent` `templates/echo-smoke.ts` template is updated in parallel.
 */
export const ECHO_SMOKE_REPLY_PREFIX = 'Okay. '

export function formatEchoSmokeReply(recognized: string): string {
  const trimmed = recognized.trim()
  if (!trimmed) {
    return ECHO_SMOKE_REPLY_PREFIX.trim()
  }
  return `${ECHO_SMOKE_REPLY_PREFIX}${trimmed}`
}

/**
 * True when the normalized transcript has at least one token before the first counting word.
 * Rejects counting-only STT (`One, two, three…`) and leading-punctuation-only (`. One…`).
 */
export function transcriptHasPrefixBeforeCounting(
  text: string,
  numberWords: readonly string[] = NUMBER_WORDS_ONE_TO_TEN,
): boolean {
  const tokens = normalizeForCompare(text)
    .split(' ')
    .filter((word) => word.length > 0)
  if (tokens.length === 0) {
    return false
  }
  const numberSet = new Set<string>(numberWords)
  const firstNumberIdx = tokens.findIndex((token) => numberSet.has(token))
  return firstNumberIdx > 0
}

export function evaluateEchoLidInboundTranscript(params: {
  recognized: string
  spokenCountingPhrase: string
  minNumberWords?: number
}): { passed: boolean; failures: string[] } {
  const failures: string[] = []
  const recognized = params.recognized.trim()
  const minNumberWords = params.minNumberWords ?? 8

  if (!recognized) {
    failures.push('Agent1 inbound: recognized transcript is empty')
    return { passed: false, failures }
  }

  if (!transcriptHasPrefixBeforeCounting(recognized)) {
    failures.push(`Agent1 inbound: local STT missing any prefix before counting: "${recognized}"`)
  }

  const numberWordsFound = countNumberWordsInTranscript(recognized, NUMBER_WORDS_ONE_TO_TEN)
  if (numberWordsFound < minNumberWords) {
    failures.push(
      `Agent1 inbound: expected at least ${minNumberWords} number words, found ${numberWordsFound}`,
    )
  }

  return { passed: failures.length === 0, failures }
}
