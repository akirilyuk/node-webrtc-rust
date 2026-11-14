/**
 * Pure helpers of `roundtrip-noisy-bystander.ts` (no models needed; Vitest in
 * `roundtrip-noisy-bystander.test.ts`).
 */

import { readFileSync } from 'fs'

import { SPEECH_EVENT_TYPE, speakerRecheckBargeEarliestMs } from '@node-webrtc-rust/sdk/voice'

import { DEFAULT_BARGE_PHRASE_MIN_SIMILARITY } from './roundtrip-barge-in-helpers.js'
import { normalizeForCompare, wordSimilarity } from './roundtrip-counting.js'

/** Product default of `speakerGate.checkSpeechMs` (G1): the verdict needs this much speech. */
export const CHECK_SPEECH_MS = 1500
/**
 * `vad.bargeIn.minSttPartialTokens` the roundtrip sets on its agent (the product default, 2). With
 * `requireSttPartial` a barge-in waits for the first partial with at least this many tokens.
 */
export const MIN_STT_PARTIAL_TOKENS = 2
/** A barge-in may come at most this long after the later of the verdict and the qualifying partial. */
export const BARGE_SLACK_MS = 300
/**
 * A barge-in may come this much before the deciding window's nominal end: one 20 ms inbound frame
 * (the gate counts the SpeechStart frame as speech) plus delivery jitter. The product bound
 * `recheck_barge_earliest_ms` (crates/speech/src/speaker_gate.rs) counts speech bytes, not
 * wall-clock.
 */
export const BARGE_EARLY_SLACK_MS = 50
/**
 * Absolute cap on barge-in delay after the caller's onset (the later of `user_speaking_start` and
 * the scene turn's start), for turns decided by the first check. A turn whose final carries `speakerRechecked: true`, or that is a merged utterance, is bounded by
 * {@link expectedBargeAtMs} alone (verdict at start + `speakerCheckWindows` × checkSpeechMs).
 */
export const BARGE_ABS_CAP_MS = 3000
export const BYSTANDER_RELATIVE_DB = -15
export const QUIET_CALLER_DBFS = -26
export const WARMUP_S = 0.6
/**
 * Anti-vacuous rule: a pass needs at least one bystander turn whose final is `other` (and that
 * did not barge in), so the gate demonstrably scored a bystander. The scene's bystanders sit 15 dB
 * under the caller, and how many of them open the VAD varies per run, so the oracle requires one
 * checked `other` bystander, not a count.
 */
export const MIN_OTHER_BYSTANDERS = 1

export interface Turn {
  role: 'main' | 'bystander'
  startMs: number
  endMs: number
  text: string
  kind: 'qualified' | 'long' | 'background'
}

export interface Seen {
  type: string
  atMs: number
  text?: string
  utteranceId?: string
  speaker?: string
  /** `user_speech_final` with `speaker: 'unknown'`: why (product field `speakerUnknownReason`). */
  speakerUnknownReason?: string
  /** `user_speech_final`: a re-check after a first `other` verdict decided it (product field). */
  speakerRechecked?: boolean
  /**
   * `user_speech_final`: check windows scored for the utterance, the first included (product field
   * `speakerCheckWindows`). A re-checked final must carry it; the oracle never defaults it.
   */
  speakerCheckWindows?: number
  /** `user_utterance_split`: the utterance that was cut (it ends with reason `speaker_split`). */
  fromUtteranceId?: string
  /** `user_utterance_split`: the utterance opened at the split point. */
  toUtteranceId?: string
  /** `user_utterance_split`: split offset into the cut utterance's audio. */
  splitAtMs?: number
  /**
   * `user_utterance_split`: media time of the cut utterance's speech onset on the agent's inbound
   * media clock (product field `fromStartMs`). `fromStartMs + splitAtMs` is the split point.
   */
  fromStartMs?: number
  /** `user_utterance_split`: speaker label before the split (product field `from`). */
  from?: string
  /** `user_utterance_split`: speaker label after the split (product field `to`). */
  to?: string
}

/**
 * A `caller` final counts as merged ({@link mergedCallerFinals}) when, past the caller phrase, it
 * holds at least this many distinct words of the bystander sentence that follows the caller turn
 * in the manifest (or every such word when that sentence has fewer).
 */
export const MERGED_FINAL_MIN_BYSTANDER_WORDS = 3
/** Shortest word that ties a final to a bystander sentence (drops "the", "and", "a"). */
export const MERGED_FINAL_MIN_WORD_LENGTH = 4
/** A final holds the caller phrase when its best caller-length window scores at least this. */
export const MERGED_FINAL_MIN_CALLER_SIMILARITY = 0.5

/**
 * Token count as the product counts it (`stt_partial_token_count` in crates/speech config.rs):
 * whitespace-separated tokens with at least one alphanumeric character.
 */
/**
 * Longest gap from a bystander turn's start to the start of the caller turn that follows it
 * (0 when no caller turn follows a bystander turn).
 */
export function maxBystanderLeadMs(turns: Turn[]): number {
  let max = 0
  for (let j = 1; j < turns.length; j++) {
    if (turns[j]!.role === 'main' && turns[j - 1]!.role === 'bystander') {
      max = Math.max(max, turns[j]!.startMs - turns[j - 1]!.startMs)
    }
  }
  return max
}

export function sttPartialTokenCount(text: string): number {
  return text.split(/\s+/).filter((tok) => tok.length > 0 && /[\p{L}\p{N}]/u.test(tok)).length
}

/**
 * When a caller's barge-in is due, from events the product emits for that utterance: the later of
 * the speaker verdict and the first `user_speech_partial` on the same `utteranceId` with at least
 * {@link MIN_STT_PARTIAL_TOKENS} tokens (`requireSttPartial`). The verdict is speech start +
 * checkSpeechMs, or, when the utterance's final says `speakerRechecked: true`, speech start +
 * `speakerRecheckBargeEarliestMs(checkSpeechMs, final.speakerCheckWindows)` (the deciding window).
 * `undefined` when the utterance never had a qualifying partial, or when a re-checked final has no
 * `speakerCheckWindows` (never defaulted; {@link evaluateTurns} reports it).
 */
export function expectedBargeAtMs(start: Seen, seen: Seen[]): number | undefined {
  const partial = qualifyingPartial(start, seen)
  if (partial == null) return undefined
  const verdictAfter = verdictAfterMs(start, seen)
  if (verdictAfter == null) return undefined
  return Math.max(start.atMs + verdictAfter, partial.atMs)
}

/** True when the start's utterance final reports a re-check (product field `speakerRechecked`). */
export function isRechecked(start: Seen, seen: Seen[]): boolean {
  return seen.some(
    (e) =>
      e.type === SPEECH_EVENT_TYPE.userSpeechFinal &&
      e.utteranceId === start.utteranceId &&
      e.speakerRechecked === true,
  )
}

/** Speech time until the verdict that releases the start's barge-in; `undefined` = no window count. */
function verdictAfterMs(start: Seen, seen: Seen[]): number | undefined {
  if (!isRechecked(start, seen)) return CHECK_SPEECH_MS
  const windows = seen.find(
    (e) =>
      e.type === SPEECH_EVENT_TYPE.userSpeechFinal &&
      e.utteranceId === start.utteranceId &&
      e.speakerRechecked === true,
  )?.speakerCheckWindows
  return windows == null ? undefined : speakerRecheckBargeEarliestMs(CHECK_SPEECH_MS, windows)
}

/** First `user_speech_partial` on the start's `utteranceId` with enough tokens for a barge-in. */
function qualifyingPartial(start: Seen, seen: Seen[]): Seen | undefined {
  return seen.find(
    (e) =>
      e.type === SPEECH_EVENT_TYPE.userSpeechPartial &&
      e.utteranceId === start.utteranceId &&
      sttPartialTokenCount(e.text ?? '') >= MIN_STT_PARTIAL_TOKENS,
  )
}

/**
 * Harness check for a caller turn without barge-in: when the agent's reply had already ended
 * (`agent_speaking_end` with no later `agent_speaking_start`) before the turn's qualifying partial,
 * there was nothing left to barge into. Returns that end and the partial, else `undefined`.
 */
export function replyEndedBeforeQualifyingPartial(
  start: Seen,
  seen: Seen[],
): { endAtMs: number; partialAtMs: number } | undefined {
  const partial = qualifyingPartial(start, seen)
  if (partial == null) return undefined
  let endAtMs: number | undefined
  for (const e of seen) {
    if (e.atMs > partial.atMs) break
    if (e.type === SPEECH_EVENT_TYPE.agentSpeakingEnd) endAtMs = e.atMs
    else if (e.type === SPEECH_EVENT_TYPE.agentSpeakingStart) endAtMs = undefined
  }
  return endAtMs != null ? { endAtMs, partialAtMs: partial.atMs } : undefined
}

/** Interleaved stereo s16le @ 48 kHz payload of a wav file (the `data` chunk). */
export function readWavPcm(path: string): Buffer {
  const buf = readFileSync(path)
  let pos = 12
  while (pos + 8 <= buf.length) {
    const id = buf.toString('ascii', pos, pos + 4)
    const size = buf.readUInt32LE(pos + 4)
    if (id === 'data') return buf.subarray(pos + 8, Math.min(buf.length, pos + 8 + size))
    pos += 8 + size + (size % 2)
  }
  throw new Error(`no data chunk in ${path}`)
}

/** Slack before the first judged turn: the agent's VAD can open a little before the voice. */
export const FIRST_SLOT_LEAD_MS = 300

/**
 * Matches each scene turn to the utterance the agent heard for it: the first unused
 * `user_speaking_start` inside the turn's slot. Slots split each gap between two turns in the
 * middle, because the VAD can open in the babble shortly before a voice starts. A turn whose
 * audio merged into the previous utterance gets none. Everything after this step pairs events by
 * `utteranceId`, never by time. Merged caller utterances are re-paired by
 * {@link pairTurnUtterances}.
 */
export function matchTurnUtterances(turns: Turn[], seen: Seen[]): (Seen | undefined)[] {
  return pairTurnUtterances(turns, seen).starts
}

/**
 * End of an utterance's speech: its `user_speaking_end`, else its `user_speech_final` (the product
 * emits both on the same `utteranceId`). `undefined` when neither arrived.
 */
export function utteranceEndMs(utteranceId: string, seen: Seen[]): number | undefined {
  const end = seen.find(
    (e) => e.type === SPEECH_EVENT_TYPE.userSpeakingEnd && e.utteranceId === utteranceId,
  )
  if (end != null) return end.atMs
  return seen.find(
    (e) => e.type === SPEECH_EVENT_TYPE.userSpeechFinal && e.utteranceId === utteranceId,
  )?.atMs
}

/**
 * Slot matching ({@link matchTurnUtterances}) plus one re-pairing that keys off product facts
 * (`oracle-relies-only-on-promised-order`): when the VAD opens in a bystander slot and the
 * utterance runs on into the next caller turn, its final says `speaker: 'caller'`. A caller
 * verdict covers the caller turn when the caller started inside the window that decided it,
 * re-checked or not (a first window that already holds the caller's start is windowed scoring).
 * Such an utterance is the caller's turn, not the bystander's, when all of these hold:
 *
 * - it was matched to a bystander turn and its final is `caller`;
 * - the next caller turn has no utterance of its own;
 * - its span covers that caller turn's scene start: speech start strictly before, and speech end
 *   (`user_speaking_end`, else the final) strictly after. An utterance that ends exactly at the
 *   caller's scene start holds no caller speech and stays with the bystander (zero gap);
 * - the deciding window reaches the caller's scene start: caller start strictly before speech
 *   start + `speakerRecheckBargeEarliestMs(checkSpeechMs, final.speakerCheckWindows)` (windows
 *   >= 1; a first-window verdict has 1). The product
 *   scores window w on speech [start + (w - 1) × checkSpeechMs, start + w × checkSpeechMs], so a
 *   deciding window that ends before the caller starts held only bystander or babble audio, and
 *   its `caller` verdict is a bystander false upgrade, not the caller's turn. A final
 *   without `speakerCheckWindows` is never paired (and {@link evaluateTurns} fails it).
 *
 * The bystander turn then has no utterance; the caller turn index goes into `merged`. Anything
 * else keeps its slot pairing, so a re-checked `caller` that never reaches a caller turn still
 * fails as a bystander barge-in.
 *
 * A second re-pairing covers an utterance that slot matching left unpaired: noise opened it in
 * a bystander slot after that slot's own utterance, and the caller's onset joined it (no new
 * speech start). It goes to the immediately following caller turn under the same final, span and
 * deciding-window conditions; the bystander turn keeps its own utterance.
 *
 * Last, a caller turn whose paired utterance was cut by an other→caller `user_utterance_split`
 * (`from: 'other'`, `to: 'caller'`, `fromUtteranceId` = its utteranceId) is judged on the split's
 * to-utterance (U2), by utteranceId: its `user_speaking_start`, final and barge-in. Other→caller
 * links are followed in a chain; a caller→other link ends it (the caller part stays on the last
 * caller to-utterance). U1's text is never credited to the turn. `followed` maps the turn index to
 * the cut utterance's id. A followed U2 is paired with that turn only: when U2's
 * `user_speaking_start` also fell into a later turn's slot, that turn takes the next unpaired start
 * in its slot instead (or none), so no U2 is judged for two turns.
 */
export function pairTurnUtterances(
  turns: Turn[],
  seen: Seen[],
): { starts: (Seen | undefined)[]; merged: Set<number>; followed: Map<number, string> } {
  const allStarts = seen.filter(
    (e) => e.type === SPEECH_EVENT_TYPE.userSpeakingStart && e.utteranceId != null,
  )
  const used = new Set<string>()
  const boundary = (i: number): number => {
    if (i <= 0) return turns[0]!.startMs - FIRST_SLOT_LEAD_MS
    if (i >= turns.length) return Number.POSITIVE_INFINITY
    return (turns[i - 1]!.endMs + turns[i]!.startMs) / 2
  }
  const starts = turns.map((_turn, i) => {
    const from = boundary(i)
    const to = boundary(i + 1)
    const start = allStarts.find((e) => !used.has(e.utteranceId!) && e.atMs >= from && e.atMs < to)
    if (start != null) used.add(start.utteranceId!)
    return start
  })
  const merged = new Set<number>()
  turns.forEach((turn, i) => {
    const start = starts[i]
    if (turn.role !== 'bystander' || start?.utteranceId == null) return
    const final = seen.find(
      (e) => e.type === SPEECH_EVENT_TYPE.userSpeechFinal && e.utteranceId === start.utteranceId,
    )
    if (final?.speaker !== 'caller') return
    const j = turns.findIndex((t, k) => k > i && t.role === 'main')
    if (j < 0 || starts[j] != null) return
    const callerStartMs = turns[j]!.startMs
    const endMs = utteranceEndMs(start.utteranceId, seen)
    if (endMs == null || !(start.atMs < callerStartMs && endMs > callerStartMs)) return
    if (final.speakerCheckWindows == null) return
    const decidedBy =
      start.atMs + speakerRecheckBargeEarliestMs(CHECK_SPEECH_MS, final.speakerCheckWindows)
    if (callerStartMs >= decidedBy) return
    starts[j] = start
    starts[i] = undefined
    merged.add(j)
  })
  // Second re-pairing: noise opened an utterance in a bystander slot after that slot's own
  // utterance, and the caller's onset joined it (STT hold-back, no new `user_speaking_start`).
  // Its start sits before the gap midpoint, so slot matching left it unpaired and the caller turn
  // empty. Same conditions as above, for an unpaired start in the bystander slot after the slot's
  // own paired utterance, credited to the next turn (which must be a caller turn).
  const pairedIds = new Set(starts.filter((s) => s != null).map((s) => s!.utteranceId!))
  for (const unpaired of allStarts) {
    if (pairedIds.has(unpaired.utteranceId!)) continue
    const i = turns.findIndex(
      (t, k) =>
        t.role === 'bystander' && unpaired.atMs >= boundary(k) && unpaired.atMs < boundary(k + 1),
    )
    if (i < 0) continue
    const own = starts[i]
    if (own == null || !(unpaired.atMs > own.atMs)) continue
    const j = i + 1
    if (turns[j]?.role !== 'main' || starts[j] != null) continue
    const final = seen.find(
      (e) => e.type === SPEECH_EVENT_TYPE.userSpeechFinal && e.utteranceId === unpaired.utteranceId,
    )
    if (final?.speaker !== 'caller' || final.speakerCheckWindows == null) continue
    const callerStartMs = turns[j]!.startMs
    const endMs = utteranceEndMs(unpaired.utteranceId!, seen)
    if (endMs == null || !(unpaired.atMs < callerStartMs && endMs > callerStartMs)) continue
    const decidedBy =
      unpaired.atMs + speakerRecheckBargeEarliestMs(CHECK_SPEECH_MS, final.speakerCheckWindows)
    if (callerStartMs >= decidedBy) continue
    starts[j] = unpaired
    pairedIds.add(unpaired.utteranceId!)
    merged.add(j)
  }
  // Follow other→caller splits from the paired utterance to its caller part, by utteranceId.
  const splits = seen.filter((e) => e.type === SPEECH_EVENT_TYPE.userUtteranceSplit)
  const followed = new Map<number, string>()
  turns.forEach((turn, i) => {
    const start = starts[i]
    if (turn.role !== 'main' || start?.utteranceId == null) return
    const visited = new Set<string>()
    let cur = start.utteranceId
    for (;;) {
      visited.add(cur)
      const next = splits.find(
        (x) => x.fromUtteranceId === cur && x.from === 'other' && x.to === 'caller',
      )
      if (next?.toUtteranceId == null || visited.has(next.toUtteranceId)) break
      cur = next.toUtteranceId
    }
    if (cur === start.utteranceId) return
    const u2Start = allStarts.find((e) => e.utteranceId === cur)
    if (u2Start == null) return
    followed.set(i, start.utteranceId)
    starts[i] = u2Start
    pairedIds.add(cur)
    starts.forEach((other, k) => {
      if (k === i || other?.utteranceId !== cur) return
      starts[k] = allStarts.find(
        (e) => !pairedIds.has(e.utteranceId!) && e.atMs >= boundary(k) && e.atMs < boundary(k + 1),
      )
      if (starts[k] != null) pairedIds.add(starts[k]!.utteranceId!)
    })
  })
  return { starts, merged, followed }
}

/**
 * The scene's similarity oracle ({@link wordSimilarity}) on the best contiguous window of the
 * recognized words, as long as the caller phrase. A merged utterance's transcript also holds the
 * babble or bystander words heard before the caller; only the caller's words are judged, and
 * they must appear together, not scattered. Same threshold, no aliases.
 */
export function callerWindowSimilarity(callerText: string, recognized: string): number {
  const n = normalizeForCompare(callerText)
    .split(' ')
    .filter((w) => w.length > 0).length
  const words = normalizeForCompare(recognized)
    .split(' ')
    .filter((w) => w.length > 0)
  if (n === 0 || words.length <= n) return wordSimilarity(callerText, recognized)
  let best = 0
  for (let k = 0; k + n <= words.length; k += 1) {
    best = Math.max(best, wordSimilarity(callerText, words.slice(k, k + n).join(' ')))
  }
  return best
}

/**
 * Length of the union of all turns (caller and bystander, scene manifest segments) clipped to
 * `[fromMs, toMs]`. Overlapping turns count once.
 */
export function voicedMsBetween(turns: Turn[], fromMs: number, toMs: number): number {
  const spans = turns
    .map((t): [number, number] => [Math.max(t.startMs, fromMs), Math.min(t.endMs, toMs)])
    .filter(([a, b]) => b > a)
    .sort((x, y) => x[0] - y[0])
  let total = 0
  let curEnd = -Infinity
  for (const [a, b] of spans) {
    const from = Math.max(a, curEnd)
    if (b > from) total += b - from
    curEnd = Math.max(curEnd, b)
  }
  return total
}

/** Last `user_speaking_end` of the utterance (a rejoined key can have several). */
export function lastSpeakingEndMs(utteranceId: string, seen: Seen[]): number | undefined {
  const ends = seen.filter(
    (e) => e.type === SPEECH_EVENT_TYPE.userSpeakingEnd && e.utteranceId === utteranceId,
  )
  return ends.length > 0 ? ends[ends.length - 1]!.atMs : undefined
}

function longWords(text: string): string[] {
  return normalizeForCompare(text)
    .split(' ')
    .filter((w) => w.length >= MERGED_FINAL_MIN_WORD_LENGTH)
}

/** A `caller` final that runs past a caller phrase into the bystander sentence after it. */
export interface MergedCallerFinal {
  utteranceId: string
  turnIndex: number
  callerText: string
  text: string
  /** Bystander words found in the final, outside the caller phrase. */
  words: string[]
}

/**
 * Finals labelled `caller` that hold a caller turn's phrase (best caller-length window at least
 * {@link MERGED_FINAL_MIN_CALLER_SIMILARITY}) and, outside that phrase's words, at least
 * {@link MERGED_FINAL_MIN_BYSTANDER_WORDS} distinct words of the bystander sentence that follows
 * the caller turn in the manifest. That is one STT decode spanning a speaker change ("nine" +
 * "Two hikers" -> "ninekers followed the rocky trail..."). With `splitOnSpeakerChange` every
 * count is a failure. Each final counts once. Finals labelled `other` or `unknown` never count.
 * Deterministic: text only, no timing.
 */
export function mergedCallerFinals(turns: Turn[], seen: Seen[]): MergedCallerFinal[] {
  const out: MergedCallerFinal[] = []
  const finals = seen.filter(
    (e) =>
      e.type === SPEECH_EVENT_TYPE.userSpeechFinal &&
      e.speaker === 'caller' &&
      e.utteranceId != null,
  )
  for (const final of finals) {
    const text = final.text ?? ''
    const finalWords = new Set(longWords(text))
    for (let i = 0; i + 1 < turns.length; i += 1) {
      const turn = turns[i]!
      const next = turns[i + 1]!
      if (turn.role !== 'main' || next.role !== 'bystander') continue
      if (callerWindowSimilarity(turn.text, text) < MERGED_FINAL_MIN_CALLER_SIMILARITY) continue
      const callerWords = new Set(longWords(turn.text))
      const bystanderWords = new Set(longWords(next.text).filter((w) => !callerWords.has(w)))
      if (bystanderWords.size === 0) continue
      const words = [...bystanderWords].filter((w) => finalWords.has(w))
      if (words.length >= Math.min(MERGED_FINAL_MIN_BYSTANDER_WORDS, bystanderWords.size)) {
        out.push({
          utteranceId: final.utteranceId!,
          turnIndex: i,
          callerText: turn.text,
          text,
          words,
        })
        break
      }
    }
  }
  return out
}

/** Number of {@link mergedCallerFinals}. */
export function countMergedCallerFinals(turns: Turn[], seen: Seen[]): number {
  return mergedCallerFinals(turns, seen).length
}

/**
 * A passing caller turn at or below this similarity is a near miss: it passed the
 * {@link DEFAULT_BARGE_PHRASE_MIN_SIMILARITY} bar with at most one word to spare. Reporting only.
 */
export const NEAR_MISS_SIMILARITY = 0.8

/** Near misses (passing similarity <= {@link NEAR_MISS_SIMILARITY}) and the minimum similarity of one pass. */
export function similaritySummary(sims: number[]): {
  nearMissCount: number
  minSimilarity: number | undefined
} {
  const eps = 1e-9
  const nearMissCount = sims.filter(
    (s) => s >= DEFAULT_BARGE_PHRASE_MIN_SIMILARITY - eps && s <= NEAR_MISS_SIMILARITY + eps,
  ).length
  const minSimilarity = sims.length > 0 ? Math.min(...sims) : undefined
  return { nearMissCount, minSimilarity }
}

/**
 * End of the window that decided an other→caller split, in scene ms: the n-th check window needs
 * n × checkSpeechMs of speech (types.ts `speakerCheckWindows`), so it ends no earlier than U1
 * `fromStartMs` + k × checkSpeechMs on the media clock, k = the cut utterance's (U1's) final
 * `speakerCheckWindows` (it includes the deciding window). `undefined` when a field is missing
 * (callers fail closed).
 */
export function splitDecidingWindowEndSceneMs(
  split: Seen | undefined,
  finals: Map<string, Seen>,
  sceneStartMediaMs: number | undefined,
): number | undefined {
  if (split?.fromStartMs == null || split.fromUtteranceId == null || sceneStartMediaMs == null) {
    return undefined
  }
  const k = finals.get(split.fromUtteranceId)?.speakerCheckWindows
  if (k == null) return undefined
  return split.fromStartMs + k * CHECK_SPEECH_MS - sceneStartMediaMs
}

/** `user_utterance_split` -> scene-clock ms of the split point (undefined when a field is missing). */
export function splitSceneMs(e: Seen, sceneStartMediaMs: number): number | undefined {
  if (e.fromStartMs == null || e.splitAtMs == null) return undefined
  return e.fromStartMs + e.splitAtMs - sceneStartMediaMs
}

export interface SplitDistance {
  fromUtteranceId?: string
  sceneSplitMs: number
  callerTurnIndex: number
  callerEndMs: number
  /** Split point minus the end of the last caller turn that started at or before it. */
  distanceMs: number
  /** First bystander turn that starts after the caller turn ends. */
  nextBystanderStartMs?: number
}

/**
 * Where each split lies against the caller's last word: the caller turn with the greatest start at
 * or before the split point. Splits without a scene time or without a caller turn before them are
 * skipped. Reporting only.
 */
export function splitDistances(
  turns: Turn[],
  seen: Seen[],
  sceneStartMediaMs: number,
): SplitDistance[] {
  const out: SplitDistance[] = []
  for (const e of seen) {
    if (e.type !== SPEECH_EVENT_TYPE.userUtteranceSplit) continue
    const sceneSplitMs = splitSceneMs(e, sceneStartMediaMs)
    if (sceneSplitMs == null) continue
    let callerTurnIndex = -1
    turns.forEach((t, i) => {
      if (t.role === 'main' && t.startMs <= sceneSplitMs) {
        if (callerTurnIndex < 0 || t.startMs > turns[callerTurnIndex]!.startMs) callerTurnIndex = i
      }
    })
    if (callerTurnIndex < 0) continue
    const callerEndMs = turns[callerTurnIndex]!.endMs
    out.push({
      fromUtteranceId: e.fromUtteranceId,
      sceneSplitMs,
      callerTurnIndex,
      callerEndMs,
      distanceMs: sceneSplitMs - callerEndMs,
      nextBystanderStartMs: turns.find((t) => t.role === 'bystander' && t.startMs > callerEndMs)
        ?.startMs,
    })
  }
  return out
}

function normWords(text: string): string[] {
  return normalizeForCompare(text)
    .split(' ')
    .filter((w) => w.length > 0)
}

/**
 * One-word glue at a speaker change, for `caller` finals of qualified caller turns that
 * {@link mergedCallerFinals} does not count. `trailing`: a word after the caller-length window
 * equals one of the first 3 words of the next bystander sentence. `leading`: a word before the
 * window equals one of the last 3 words of the previous bystander turn. Reporting only.
 */
export function glueCounts(turns: Turn[], seen: Seen[]): { trailing: number; leading: number } {
  const mergedIds = new Set(mergedCallerFinals(turns, seen).map((m) => m.utteranceId))
  const { starts } = pairTurnUtterances(turns, seen)
  let trailing = 0
  let leading = 0
  turns.forEach((turn, i) => {
    if (turn.role !== 'main' || turn.kind !== 'qualified') return
    const id = starts[i]?.utteranceId
    if (id == null || mergedIds.has(id)) return
    const final = [...seen]
      .reverse()
      .find((e) => e.type === SPEECH_EVENT_TYPE.userSpeechFinal && e.utteranceId === id)
    if (final?.speaker !== 'caller') return
    const callerWords = normWords(turn.text)
    const words = normWords(final.text ?? '')
    const n = callerWords.length
    if (n === 0 || words.length <= n) return
    let bestK = 0
    let best = -1
    for (let k = 0; k + n <= words.length; k += 1) {
      const sim = wordSimilarity(turn.text, words.slice(k, k + n).join(' '))
      if (sim > best) {
        best = sim
        bestK = k
      }
    }
    const before = words.slice(0, bestK)
    const after = words.slice(bestK + n)
    let next: Turn | undefined
    for (let j = i + 1; j < turns.length && next == null; j += 1) {
      if (turns[j]!.role === 'bystander') next = turns[j]
    }
    let prev: Turn | undefined
    for (let j = i - 1; j >= 0 && prev == null; j -= 1) {
      if (turns[j]!.role === 'bystander') prev = turns[j]
    }
    if (next != null) {
      const head = normWords(next.text).slice(0, 3)
      if (after.some((w) => head.includes(w))) trailing += 1
    }
    if (prev != null) {
      const tail = normWords(prev.text).slice(-3)
      if (before.some((w) => tail.includes(w))) leading += 1
    }
  })
  return { trailing, leading }
}

/** Float tolerance for "exactly at the threshold". */
export const EXACT_THRESHOLD_EPS = 1e-9

/** Similarities that sit exactly on {@link DEFAULT_BARGE_PHRASE_MIN_SIMILARITY} (0.75). Reporting only. */
export function exactThresholdCount(sims: number[]): number {
  return sims.filter(
    (s) => Math.abs(s - DEFAULT_BARGE_PHRASE_MIN_SIMILARITY) <= EXACT_THRESHOLD_EPS,
  ).length
}

/** Pure evaluation of one pass; exported for the evaluator unit test. */
export function evaluateTurns(
  turns: Turn[],
  seen: Seen[],
  label: string,
  opts: { sceneStartMediaMs?: number } = {},
): string[] {
  const failures: string[] = []
  const sims: number[] = []
  const finals = new Map<string, Seen>()
  for (const e of seen) {
    if (e.type === SPEECH_EVENT_TYPE.userSpeechFinal && e.utteranceId != null) {
      finals.set(e.utteranceId, e)
    }
  }
  const barges = seen.filter((e) => e.type === SPEECH_EVENT_TYPE.bargeIn)
  // A bystander never barges in: any barge_in whose utterance's final is labelled other fails,
  // whether or not that utterance matched a scene turn.
  for (const b of barges) {
    if (b.utteranceId != null && finals.get(b.utteranceId)?.speaker === 'other') {
      failures.push(
        `${label}: barge_in at ${b.atMs} ms for utterance ${b.utteranceId} whose final is speaker other`,
      )
    }
  }
  for (const b of barges) {
    if (b.utteranceId == null)
      failures.push(`${label}: barge_in at ${b.atMs} ms without utteranceId`)
  }
  const bargesOf = (id: string): Seen[] => barges.filter((b) => b.utteranceId === id)
  // A re-checked final must say how many windows decided it; the oracle never defaults it.
  for (const f of finals.values()) {
    if (f.speakerRechecked === true && f.speakerCheckWindows == null) {
      failures.push(
        `${label}: re-checked final has no speakerCheckWindows (utterance ${f.utteranceId})`,
      )
    }
  }
  const { starts, merged, followed } = pairTurnUtterances(turns, seen)
  const splits = seen.filter((e) => e.type === SPEECH_EVENT_TYPE.userUtteranceSplit)
  let bystandersHeard = 0
  turns.forEach((turn, i) => {
    const tag = `${label} turn ${i} (${turn.role} ${turn.kind} "${turn.text.slice(0, 30)}")`
    const start = starts[i]
    const id = start?.utteranceId
    const final = id != null ? finals.get(id) : undefined
    if (turn.role === 'bystander') {
      // Documented behaviour of minCheckSpeechMs (1500) with suppressShortBargeIn off: an utterance
      // with less speech is not scored (`unknown`, reason `short`) and its barge-in is released at
      // its end. Any other unknown, or an unknown without a reason, is a failure.
      const shortUnknown = final?.speaker === 'unknown' && final.speakerUnknownReason === 'short'
      if (id != null && bargesOf(id).length > 0 && !shortUnknown) {
        failures.push(`${tag}: barge_in for the bystander utterance ${id}`)
      }
      if (final != null) {
        if (final.speaker === 'other') {
          if (id == null || bargesOf(id).length === 0) bystandersHeard += 1
        } else if (shortUnknown) {
          console.log(`${tag}: too short to check (unknown, reason short)`)
        } else {
          failures.push(
            `${tag}: final speaker ${String(final.speaker)}${final.speakerUnknownReason != null ? ` (reason ${final.speakerUnknownReason})` : ''}, expected other`,
          )
        }
      }
      return
    }
    if (start == null || id == null) {
      // A caller turn needs a caller-labelled final of its own. Speech inside an earlier
      // utterance no longer counts: the agent splits that utterance at the speaker change
      // (`speakerGate.splitOnSpeakerChange`).
      failures.push(`${tag}: caller speech start not detected`)
      return
    }
    const isMerged = merged.has(i)
    const cutId = followed.get(i)
    if (cutId != null) {
      console.log(
        `${tag}: other→caller split ${cutId} -> ${id}; judged on ${id} (final, speaker, barge-in)`,
      )
      // The caller's part is U2 alone: no word of the bystander sentence before this caller turn
      // (U1's speaker) may land in U2's caller final.
      const prev = turns
        .slice(0, i)
        .reverse()
        .find((t) => t.role === 'bystander')
      if (prev != null && final != null) {
        const callerWords = new Set(longWords(turn.text))
        const finalWords = new Set(longWords(final.text ?? ''))
        const leaked = [
          ...new Set(longWords(prev.text).filter((w) => !callerWords.has(w) && finalWords.has(w))),
        ]
        if (leaked.length > 0) {
          failures.push(
            `${tag}: caller final ${id} after the other→caller split of ${cutId} holds bystander words (${leaked.join(' ')}): "${final.text ?? ''}"`,
          )
        }
      }
    }
    if (isMerged) {
      console.log(
        `${tag}: merged utterance ${id} (started ${start.atMs} ms, before the caller at ${turn.startMs} ms; final caller, ${isRechecked(start, seen) ? 're-checked' : 'first window'}, windows ${String(final?.speakerCheckWindows)})`,
      )
    }
    const barge = bargesOf(id)[0]
    if (barge == null) {
      // Still a failure, never a skip: a harness miss is named so a real miss cannot hide behind it.
      const ended = replyEndedBeforeQualifyingPartial(start, seen)
      failures.push(
        ended != null
          ? `${tag}: harness: reply ended at ${ended.endAtMs} ms before the qualifying partial at ${ended.partialAtMs} ms (lengthen AGENT_PHRASE)`
          : `${tag}: no barge_in`,
      )
    } else {
      const delay = barge.atMs - start.atMs
      const expected = expectedBargeAtMs(start, seen)
      console.log(
        `${tag}: barge-in ${delay} ms after speech start, ${expected != null ? barge.atMs - expected : 'n/a'} ms after expected`,
      )
      if (expected == null && isRechecked(start, seen) && verdictAfterMs(start, seen) == null) {
        // Reported once per final above.
      } else if (expected == null) {
        failures.push(
          `${tag}: barge_in without a qualifying partial (>= ${MIN_STT_PARTIAL_TOKENS} tokens) on ${id}`,
        )
      } else if (isRechecked(start, seen) || isMerged) {
        // A merged utterance (opened on bystander or babble speech) is bounded like a re-checked one: its barge waits on a qualifying partial of a turn that started late, so the 3 s cap from the caller's onset does not apply.
        // The gate counts every frame while its VAD segment is open (speech, babble, pauses under minSilenceDurationMs; not the pre-roll) and restarts at the last window boundary when a segment rejoins the key after SpeechEnd. Voiced scene time is therefore usually at most the counted speech, so this check understates lateness; it can overstate it only after a rejoin. The speech-end and final checks below bound a barge after the audio stops.
        const windowEnd = verdictAfterMs(start, seen)!
        const late = voicedMsBetween(turns, start.atMs, barge.atMs) - windowEnd
        if (isMerged && !isRechecked(start, seen)) {
          // merged, not re-checked (a re-checked merged utterance keeps the voiced-audio bound below): the barge waits on the first qualifying STT partial of an utterance that began on bystander speech; partial lag is tracked in followups/2026-11-02-speaker-gate-mostly-bystander-first-window.md
          if (barge.atMs - expected > BARGE_SLACK_MS) {
            failures.push(
              `${tag}: barge-in ${barge.atMs - expected} ms after expected (max of verdict and qualifying partial) > ${BARGE_SLACK_MS}`,
            )
          }
        } else if (late > BARGE_SLACK_MS) {
          failures.push(
            `${tag}: barge ${late} ms of voiced scene audio after the deciding window (> ${BARGE_SLACK_MS} ms)`,
          )
        }
        const endAt = lastSpeakingEndMs(id!, seen) ?? final?.atMs
        if (endAt != null && barge.atMs - endAt > BARGE_SLACK_MS) {
          failures.push(
            `${tag}: barge ${barge.atMs - endAt} ms after the utterance's speech end at ${endAt} ms (> ${BARGE_SLACK_MS} ms; no window is scored after SpeechEnd)`,
          )
        }
        if (final != null && barge.atMs > final.atMs) {
          failures.push(`${tag}: barge-in at ${barge.atMs} ms after its final at ${final.atMs} ms`)
        }
      } else if (barge.atMs - expected > BARGE_SLACK_MS) {
        failures.push(
          `${tag}: barge-in ${barge.atMs - expected} ms after expected (max of verdict and qualifying partial) > ${BARGE_SLACK_MS}`,
        )
      }
      // A merged utterance started before the caller spoke: a barge-in before the caller's scene
      // start was released on the bystander or babble alone.
      if (isMerged && barge.atMs <= turn.startMs) {
        failures.push(
          `${tag}: barge-in at ${barge.atMs} ms on merged utterance ${id}, not after the caller's scene start ${turn.startMs} ms`,
        )
      }
      // A barge-in released by the deciding window cannot come before that window ends. U2 of an
      // other→caller split inherits the windows scored before the cut (its speech start event is
      // emitted at the split, after that audio), so its bound is anchored on U1: the end of the
      // window that decided the split (U1 `fromStartMs` + U1 `speakerCheckWindows` x
      // checkSpeechMs, scene time). A missing field fails closed.
      if (cutId != null) {
        const split = splits.find(
          (x) => x.toUtteranceId === id && x.from === 'other' && x.to === 'caller',
        )
        const end = splitDecidingWindowEndSceneMs(split, finals, opts.sceneStartMediaMs)
        if (end == null) {
          failures.push(
            `${tag}: other→caller split ${String(split?.fromUtteranceId)} -> ${id}: deciding window end unknown (fromStartMs, U1 speakerCheckWindows or sceneStartMediaMs missing)`,
          )
        } else if (barge.atMs < end - BARGE_EARLY_SLACK_MS) {
          failures.push(
            `${tag}: barge-in at ${barge.atMs} ms on ${id}, before the split's deciding window ends at ${end} ms (U1 ${String(split?.fromUtteranceId)}, slack ${BARGE_EARLY_SLACK_MS} ms)`,
          )
        }
      }
      const decided =
        isRechecked(start, seen) && cutId == null ? verdictAfterMs(start, seen) : undefined
      if (decided != null && delay < decided - BARGE_EARLY_SLACK_MS) {
        failures.push(
          `${tag}: barge-in ${delay} ms after speech start, before the deciding window ends at ${decided} ms (slack ${BARGE_EARLY_SLACK_MS} ms)`,
        )
      }
      // The VAD can open on babble or a bystander tail before the caller speaks, so speech start is
      // not the caller's onset; the cap bounds the caller-perceived latency.
      const capFrom = Math.max(start.atMs, turn.startMs)
      if (barge.atMs - capFrom > BARGE_ABS_CAP_MS && !isRechecked(start, seen) && !isMerged) {
        failures.push(
          `${tag}: barge-in ${barge.atMs - capFrom} ms after the caller's onset (max of speech start and scene turn start) > cap ${BARGE_ABS_CAP_MS}`,
        )
      }
    }
    if (final == null) {
      failures.push(`${tag}: no user_speech_final`)
      return
    }
    if (final.speaker !== 'caller') {
      failures.push(`${tag}: final speaker ${String(final.speaker)}, expected caller`)
    }
    if (turn.kind === 'qualified') {
      const sim = isMerged
        ? callerWindowSimilarity(turn.text, final.text ?? '')
        : wordSimilarity(turn.text, final.text ?? '')
      sims.push(sim)
      console.log(
        `${tag}: recognized "${final.text ?? ''}" similarity ${sim.toFixed(2)}${isMerged ? ' (best caller-length window)' : ''}`,
      )
      if (sim < DEFAULT_BARGE_PHRASE_MIN_SIMILARITY) {
        failures.push(`${tag}: recognized "${final.text ?? ''}" similarity ${sim.toFixed(2)}`)
      }
    }
  })
  for (const e of splits) {
    console.log(
      `${label}: user_utterance_split ${String(e.fromUtteranceId)} -> ${String(e.toUtteranceId)} at ${String(e.splitAtMs)} ms`,
    )
  }
  const mergedFinals = mergedCallerFinals(turns, seen)
  const { nearMissCount, minSimilarity } = similaritySummary(sims)
  const distances =
    opts.sceneStartMediaMs != null ? splitDistances(turns, seen, opts.sceneStartMediaMs) : []
  for (const e of splits) {
    const d = distances.find((x) => x.fromUtteranceId === e.fromUtteranceId)
    console.log(
      `${label}: split ${String(e.fromUtteranceId)} sceneSplitMs=${d?.sceneSplitMs ?? 'n/a'} callerTurn=${d?.callerTurnIndex ?? 'n/a'} callerEndMs=${d?.callerEndMs ?? 'n/a'} distanceMs=${d?.distanceMs ?? 'n/a'} nextBystanderStartMs=${d?.nextBystanderStartMs ?? 'n/a'}`,
    )
  }
  const distMin = distances.length > 0 ? Math.min(...distances.map((d) => d.distanceMs)) : undefined
  const distMax = distances.length > 0 ? Math.max(...distances.map((d) => d.distanceMs)) : undefined
  const glue = glueCounts(turns, seen)
  console.log(
    `${label}: user_utterance_split events: ${splits.length}; merged caller finals: ${mergedFinals.length}; near misses (similarity <= ${NEAR_MISS_SIMILARITY.toFixed(2)}): ${nearMissCount}; min similarity: ${minSimilarity == null ? 'n/a' : minSimilarity.toFixed(2)}; sceneStartMediaMs=${opts.sceneStartMediaMs ?? 'n/a'}; split distance ms min/max: ${distMin ?? 'n/a'}/${distMax ?? 'n/a'}; trailing glue finals: ${glue.trailing}; leading glue finals: ${glue.leading}; exact-threshold (0.75) finals: ${exactThresholdCount(sims)}`,
  )
  for (const m of mergedFinals) {
    failures.push(
      `${label}: merged caller final ${m.utteranceId} (turn ${m.turnIndex} "${m.callerText}" runs into the next bystander: ${m.words.join(' ')}): "${m.text}"`,
    )
  }
  console.log(`${label}: bystander finals tagged other: ${bystandersHeard}`)
  if (bystandersHeard < MIN_OTHER_BYSTANDERS) {
    failures.push(
      `${label}: only ${bystandersHeard} bystander utterance(s) tagged other without a barge-in, need at least ${MIN_OTHER_BYSTANDERS} (vacuous pass)`,
    )
  }
  return failures
}
