/**
 * Guard for error class `test.wrong-oracle`: a roundtrip phrase that the STT judges must have been
 * qualified against the recognizer (hundreds of fresh Piper syntheses, recognized offline), so a CI
 * failure on it means the product broke, not that the phrase is misheard now and then.
 *
 * The table `stt-phrase-qualification.json` comes from the `phrase_sweep` example
 * (`crates/vendor-sherpa-onnx/examples/phrase_sweep.rs`).
 */
import { readdirSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

import {
  DEFAULT_BARGE_PHRASE_MIN_SIMILARITY,
  DEFAULT_BARGE_TTS_PHRASE,
} from './roundtrip-barge-in-helpers.js'
import { LEG_CATALOG } from './roundtrip-concurrent-timing-helpers.js'
import {
  DEFAULT_BARGE_PHRASE,
  DEFAULT_RECOVERY_PHRASE,
} from './roundtrip-counting-barge-recovery.js'
import {
  DEFAULT_ECHO_MIN_SIMILARITY,
  DEFAULT_LONG_SENTENCE_PHRASE,
  DEFAULT_MIN_SIMILARITY as LONG_SENTENCE_LEG_A_MIN_SIMILARITY,
  ECHO_REPLY_PREFIX,
} from './roundtrip-counting-echo.js'
import {
  DEFAULT_PHRASE as TTS_STREAM_PHRASE,
  DEFAULT_MIN_SIMILARITY as TTS_STREAM_MIN_SIMILARITY,
  REQUIRED_WORDS as TTS_STREAM_REQUIRED_WORDS,
} from './roundtrip-tts-stream.js'
import { DEFAULT_PHRASE_TWO, PHRASE_TWO_KEYWORD } from './roundtrip-two-phrases.js'

const SRC_DIR = dirname(fileURLToPath(import.meta.url))
const EXAMPLE_DIR = join(SRC_DIR, '..')
/**
 * Zero misses in 3000 syntheses bounds a phrase's miss rate below ~0.1 % (rule of three, 95 %).
 * 300 was not enough: "stop now please" was clean in 300 and misheard as "Sop now please" in 4 of
 * 3000, which failed the barge-in roundtrips in CI and a release.
 */
const MIN_SYNTHESES = 3000

/** `roundtrip-counting-echo.ts` `DEFAULT_ECHO_MIN_SIMILARITY` / `SHERPA_ECHO_LEG_MIN_SIMILARITY`. */
const ECHO_LEG_MIN_SIMILARITY = 0.6
/** `roundtrip-counting-barge-recovery.ts` `roundParams.minSimilarity` (recovery leg A). */
const RECOVERY_LEG_A_MIN_SIMILARITY = 0.75

interface QualifiedPhrase {
  phrase: string
  n: number
  minObserved: number
  keyword: string | null
  keywordMissing: number | null
}

interface QualificationTable {
  stt: string
  tts: string
  phrases: QualifiedPhrase[]
}

/**
 * `threshold`: minimum word similarity; `keyword`: word(s) every transcript must contain as whole
 * tokens (several joined with `,`, as `phrase_sweep` writes them for repeated `--keyword`).
 */
type Judged =
  | { phrase: string; threshold: number; keyword?: string }
  | { phrase: string; threshold?: number; keyword: string }

const table = JSON.parse(
  readFileSync(join(EXAMPLE_DIR, 'stt-phrase-qualification.json'), 'utf8'),
) as QualificationTable

/** Every phrase whose recognized text decides a CI roundtrip, with how it is judged. */
const JUDGED: Judged[] = [
  // Recovery round: leg A must score the recovery phrase, leg B the "You said: ..." echo.
  { phrase: DEFAULT_RECOVERY_PHRASE, threshold: RECOVERY_LEG_A_MIN_SIMILARITY },
  { phrase: `You said: ${DEFAULT_RECOVERY_PHRASE}`, threshold: ECHO_LEG_MIN_SIMILARITY },
  // Barge phrases (barge-in, barge replay, counting barge recovery).
  { phrase: DEFAULT_BARGE_TTS_PHRASE, threshold: DEFAULT_BARGE_PHRASE_MIN_SIMILARITY },
  { phrase: DEFAULT_BARGE_PHRASE, threshold: DEFAULT_BARGE_PHRASE_MIN_SIMILARITY },
  // Two-phrase roundtrip: phase 2 passes when the final contains its keyword (`textIncludes`).
  { phrase: DEFAULT_PHRASE_TWO, keyword: PHRASE_TWO_KEYWORD },
  // Echo long-sentence round: leg A scores the phrase, leg B the "You said: ..." echo. nwr speaks
  // it as two engine calls (two sentences); phrase_sweep makes the same calls.
  { phrase: DEFAULT_LONG_SENTENCE_PHRASE, threshold: LONG_SENTENCE_LEG_A_MIN_SIMILARITY },
  {
    phrase: `${ECHO_REPLY_PREFIX}${DEFAULT_LONG_SENTENCE_PHRASE}`,
    threshold: DEFAULT_ECHO_MIN_SIMILARITY,
  },
  // TTS stream roundtrip: similarity on both legs plus required words.
  {
    phrase: TTS_STREAM_PHRASE,
    threshold: TTS_STREAM_MIN_SIMILARITY,
    keyword: TTS_STREAM_REQUIRED_WORDS.join(','),
  },
  // The 3 legs CI runs in the concurrent multi-client roundtrip: judged by keyword only.
  ...LEG_CATALOG.slice(0, 3).map(({ phrase, keyword }) => ({ phrase, keyword })),
]

/**
 * Phrase constants that the STT does not judge by word similarity (or that are not qualified yet).
 * Keyed by the string literal the constant starts with.
 */
const UNQUALIFIED_PHRASES: Record<string, string> = {
  'one two three four five six seven eight nine ten':
    'counting phrase, judged by number-word counts (>= 8 of 10), not by word similarity',
  'one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty':
    'counting phrase, judged by number-word counts, not by word similarity',
  'The quick brown fox jumps over the lazy dog and then continues speaking for several more seconds so we can interrupt playback.':
    'agent speech that barge-in cuts off; judged by playback duration ratios, its transcript is not compared',
}

interface PhraseConstant {
  name: string
  value: string
}

/**
 * `const <NAME> = '<literal>'` (or double quotes) where NAME contains PHRASE and the literal has a
 * space. The literal may start on the line after `=`; a concatenation is reported by its first part.
 */
export function findPhraseConstants(source: string): PhraseConstant[] {
  const found: PhraseConstant[] = []
  const pattern = /\bconst\s+(\w*PHRASE\w*)\s*(?::[^=\n]+)?=\s*(['"])((?:(?!\2)[^\n\\])*)\2/g
  for (const match of source.matchAll(pattern)) {
    const [, name, , value] = match
    if (name && value && value.includes(' ')) found.push({ name, value })
  }
  return found
}

function sweepCommand(item: Judged): string {
  const keyword = (item.keyword?.split(',') ?? []).map((word) => ` --keyword ${word}`).join('')
  return (
    'cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example phrase_sweep -- ' +
    `--stt <stt-model-dir> --tts <tts-model-dir> --n ${MIN_SYNTHESES}${keyword} "${item.phrase}"`
  )
}

function defaultModelBundles(): { stt: string; tts: string } {
  const script = readFileSync(
    join(EXAMPLE_DIR, '..', '..', 'scripts', 'export-sherpa-local-models.sh'),
    'utf8',
  )
  const stt = /^STT_BUNDLE="\$\{SHERPA_STT_BUNDLE:-([^}"]+)\}"/m.exec(script)?.[1]
  const tts = /^TTS_BUNDLE="\$\{SHERPA_TTS_BUNDLE:-([^}"]+)\}"/m.exec(script)?.[1]
  if (!stt || !tts)
    throw new Error('default STT_BUNDLE / TTS_BUNDLE not found in export-sherpa-local-models.sh')
  return { stt, tts }
}

describe('STT phrase qualification', () => {
  it('every STT-judged roundtrip phrase is qualified', () => {
    for (const item of JUDGED) {
      const entry = table.phrases.find((p) => p.phrase === item.phrase)
      const hint = `phrase ${JSON.stringify(item.phrase)} is judged by a roundtrip but not qualified; run: ${sweepCommand(item)} and add the output to stt-phrase-qualification.json`
      expect(entry, hint).toBeDefined()
      expect(entry!.n, `${hint} (n < ${MIN_SYNTHESES})`).toBeGreaterThanOrEqual(MIN_SYNTHESES)
      if (item.keyword !== undefined) {
        expect(entry!.keyword, `${hint} (swept without --keyword ${item.keyword})`).toBe(
          item.keyword,
        )
        expect(
          entry!.keywordMissing,
          `phrase ${JSON.stringify(item.phrase)}: keyword "${item.keyword}" missing in ${entry!.keywordMissing} of ${entry!.n} syntheses; pick another phrase, then run: ${sweepCommand(item)}`,
        ).toBe(0)
      }
      if (item.threshold !== undefined) {
        expect(
          entry!.minObserved,
          `phrase ${JSON.stringify(item.phrase)}: min similarity ${entry!.minObserved} < ${item.threshold}; pick another phrase, then run: ${sweepCommand(item)}`,
        ).toBeGreaterThanOrEqual(item.threshold)
      }
    }
  })

  it('table was produced for the models CI uses', () => {
    const defaults = defaultModelBundles()
    expect(table.stt).toBe(defaults.stt)
    expect(table.tts).toBe(defaults.tts)
  })

  it('new phrase constants are qualified or allowlisted', () => {
    const qualified = new Set(table.phrases.map((p) => p.phrase))
    const unknown: string[] = []
    const seenAllowlisted = new Set<string>()
    for (const file of readdirSync(SRC_DIR)) {
      if (!file.endsWith('.ts') || file.endsWith('.test.ts')) continue
      for (const { name, value } of findPhraseConstants(
        readFileSync(join(SRC_DIR, file), 'utf8'),
      )) {
        if (qualified.has(value)) continue
        if (value in UNQUALIFIED_PHRASES) {
          seenAllowlisted.add(value)
          continue
        }
        unknown.push(`${file}: ${name} = ${JSON.stringify(value)}`)
      }
    }
    expect(
      unknown,
      'phrase constants neither in stt-phrase-qualification.json nor in UNQUALIFIED_PHRASES (qualify with phrase_sweep, or allowlist with a reason)',
    ).toEqual([])
    const stale = Object.keys(UNQUALIFIED_PHRASES).filter((value) => !seenAllowlisted.has(value))
    expect(stale, 'UNQUALIFIED_PHRASES entries no source constant uses any more').toEqual([])
  })

  it('the scan reports a new unknown phrase constant', () => {
    const found = findPhraseConstants("export const NEW_PHRASE = 'some new words'\n")
    expect(found).toEqual([{ name: 'NEW_PHRASE', value: 'some new words' }])
    const qualified = new Set(table.phrases.map((p) => p.phrase))
    expect(
      found.filter(({ value }) => !qualified.has(value) && !(value in UNQUALIFIED_PHRASES)),
    ).toHaveLength(1)
  })

  it('the scan reads a literal on the next line and ignores single words', () => {
    expect(
      findPhraseConstants("const A_PHRASE =\n  'two words'\nconst B_PHRASE = 'single'\n"),
    ).toEqual([{ name: 'A_PHRASE', value: 'two words' }])
  })
})
