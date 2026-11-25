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

export interface QualifiedPhrase {
  phrase: string
  n: number
  minObserved: number
  keyword: string | null
  keywordMissing: number | null
  /** `phrase_sweep --lead-range-ms`: silence before the clip, drawn per synthesis (ms). */
  leadRangeMs?: [number, number]
  leadSeed?: number
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
  'The quick brown fox jumps over the lazy dog and then keeps on talking about the weather in the valley. ':
    'noisy-bystander agent reply (first part of a concatenation); only its playback span is used, its transcript is never compared',
  'Hello, I am ready to help.':
    'noisy-bystander warm-up reply (WARM_REPLY_PHRASE, decision ii): agent TTS before the scene anchor; only its agent_speaking_end is awaited, its transcript is never recognized or compared',
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

/**
 * Where the live agent's STT stream puts the caller's onset (`AGENT_ONSET_RANGE_MS` in
 * `phrase_sweep.rs`: pre-roll padded to capacity by `drain_padded_to_capacity`, minus the VAD
 * delay). The recognizer's reading of a first word depends on that position: "excuse me please"
 * was clean in 3000 at the old fixed 300 ms lead and misheard as "X accuse me, please" at 525 ms
 * (release 0.23.1 gate). A qualification at 300 ms only does not count.
 */
export function agentOnsetRangeMs(source: string): [number, number] {
  const m = /const AGENT_ONSET_RANGE_MS: \(usize, usize\) = \((\d+), (\d+)\);/.exec(source)
  if (!m) throw new Error('AGENT_ONSET_RANGE_MS not found in phrase_sweep.rs')
  return [Number(m[1]), Number(m[2])]
}

/**
 * Why a judged clean record does not count as qualified at the agent's onset, or null when it does:
 * it must be swept over exactly the agent onset range, with the seed recorded.
 */
export function onsetQualificationDefect(
  e: QualifiedPhrase,
  range: [number, number],
): string | null {
  if (!e.leadRangeMs || e.leadRangeMs[0] !== range[0] || e.leadRangeMs[1] !== range[1])
    return `not swept over the agent onset range ${range.join('-')} ms`
  if (typeof e.leadSeed !== 'number') return 'no leadSeed recorded'
  return null
}

const AGENT_ONSET_RANGE = agentOnsetRangeMs(
  readFileSync(
    join(EXAMPLE_DIR, '..', '..', 'crates', 'vendor-sherpa-onnx', 'examples', 'phrase_sweep.rs'),
    'utf8',
  ),
)

function sweepCommand(item: Judged): string {
  const keyword = (item.keyword?.split(',') ?? []).map((word) => ` --keyword ${word}`).join('')
  return (
    'cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example phrase_sweep -- ' +
    `--stt <stt-model-dir> --tts <tts-model-dir> --n ${MIN_SYNTHESES} --lead-range-ms agent --seed <n>${keyword} "${item.phrase}"`
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
      expect(onsetQualificationDefect(entry!, AGENT_ONSET_RANGE), hint).toBeNull()
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

  it('reads the agent onset range from phrase_sweep.rs', () => {
    expect(AGENT_ONSET_RANGE).toEqual([450, 650])
    expect(AGENT_ONSET_RANGE[0]).toBeLessThanOrEqual(525)
    expect(AGENT_ONSET_RANGE[1]).toBeGreaterThanOrEqual(525)
    expect(() => agentOnsetRangeMs('const LEAD_SILENCE_MS: usize = 300;')).toThrow()
  })

  it('onset qualification refuses legacy, 300 ms and seedless records', () => {
    const base: QualifiedPhrase = {
      phrase: 'x',
      n: 3000,
      minObserved: 1,
      keyword: null,
      keywordMissing: null,
    }
    const range: [number, number] = [450, 650]
    expect(onsetQualificationDefect(base, range)).toContain('not swept')
    expect(
      onsetQualificationDefect({ ...base, leadRangeMs: [300, 300], leadSeed: 1 }, range),
    ).toContain('not swept')
    expect(onsetQualificationDefect({ ...base, leadRangeMs: [450, 650] }, range)).toBe(
      'no leadSeed recorded',
    )
    expect(
      onsetQualificationDefect({ ...base, leadRangeMs: [450, 650], leadSeed: 20261125 }, range),
    ).toBeNull()
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

/**
 * Babble sweeps (`phrase_sweep --babble`): the noisy-bystander roundtrip judges its caller phrases
 * over background babble 15 dB below the caller, a condition the clean sweep does not cover
 * ("excuse me please" was clean in 3000 clean runs and heard as "X cuse me, please." in that scene).
 * `speaker_fixtures` only draws S3 caller phrases from entries with a clean babble sweep (guarded in
 * `speaker_fixtures.rs` › `noisy_bystander_scene_judges_only_babble_clean_phrases`); this checks the
 * recorded sweeps used the scene's conditions and its oracle.
 */
describe('babble qualification (noisy-bystander scene)', () => {
  /** Caller levels of `s3_test_m15` and `s3_test_m15_l26` (dBFS). */
  const SCENE_CALLER_LEVELS = [-20, -26]
  const MIN_BABBLE_ROLLS = 1000
  /** 4 qualified caller turns per scene (16 turns, every other caller turn is a long sentence). */
  const SCENE_JUDGED_TURNS = 4

  interface BabbleLevel {
    callerDb: number
    n: number
    misses: number
  }
  interface BabbleEntry {
    phrase: string
    babble?: { relDb: number; threshold: number; levels: BabbleLevel[] }
  }
  const entries = table.phrases as unknown as BabbleEntry[]
  const swept = entries.filter((e) => e.babble !== undefined)
  const clean = swept.filter((e) =>
    SCENE_CALLER_LEVELS.every((db) =>
      e.babble!.levels.some((l) => l.callerDb === db && l.n >= MIN_BABBLE_ROLLS && l.misses === 0),
    ),
  )

  it('sweeps ran under the scene conditions and scored with the scene oracle', () => {
    for (const e of swept) {
      expect(e.babble!.relDb, e.phrase).toBe(-15)
      expect(e.babble!.threshold, e.phrase).toBe(DEFAULT_BARGE_PHRASE_MIN_SIMILARITY)
      expect(e.babble!.levels.map((l) => l.callerDb).sort(), e.phrase).toEqual(
        [...SCENE_CALLER_LEVELS].sort(),
      )
    }
  })

  it('enough babble-clean short phrases for the scene to judge', () => {
    const short = clean.filter((e) => e.phrase.split(/\s+/).length <= 12)
    expect(short.length).toBeGreaterThanOrEqual(SCENE_JUDGED_TURNS)
  })
})
