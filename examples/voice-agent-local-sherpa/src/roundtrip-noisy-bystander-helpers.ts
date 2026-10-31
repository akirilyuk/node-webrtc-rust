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
 * the scene turn's start), for turns decided by the first check. A turn whose final carries `speakerRechecked: true` is bounded by
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
}

/** Plan of the temporary absorb rule ({@link pairTurnUtterances}). */
export const ABSORB_PLAN =
  'development/node-webrtc-rust/plans/2026-10-31-speaker-gate-continuous-recheck.md'
/** The run fails when more than this share of caller turns is absorbed into an earlier utterance. */
export const MAX_ABSORBED_SHARE = 0.25

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

/** True when the agent was speaking at `atMs`: its last speaking event up to then is a start. */
function agentSpeakingAt(atMs: number, seen: Seen[]): boolean {
  let speaking = false
  for (const e of seen) {
    if (e.atMs > atMs) break
    if (e.type === SPEECH_EVENT_TYPE.agentSpeakingStart) speaking = true
    else if (e.type === SPEECH_EVENT_TYPE.agentSpeakingEnd) speaking = false
  }
  return speaking
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
 * utterance runs on into the next caller turn, the product re-checks it and its final says
 * `speakerRechecked: true`, `speaker: 'caller'`. Such an utterance is the caller's turn, not the
 * bystander's, when all of these hold:
 *
 * - it was matched to a bystander turn and its final is re-checked `caller`;
 * - the next caller turn has no utterance of its own;
 * - its span covers that caller turn's scene start: speech start strictly before, and speech end
 *   (`user_speaking_end`, else the final) strictly after. An utterance that ends exactly at the
 *   caller's scene start holds no caller speech and stays with the bystander (zero gap);
 * - the deciding window reaches the caller's scene start: caller start strictly before speech
 *   start + `speakerRecheckBargeEarliestMs(checkSpeechMs, final.speakerCheckWindows)`. The product
 *   scores window w on speech [start + (w - 1) × checkSpeechMs, start + w × checkSpeechMs], so a
 *   deciding window that ends before the caller starts held only bystander or babble audio, and
 *   its `caller` verdict is a bystander false upgrade, not the caller's turn. A re-checked final
 *   without `speakerCheckWindows` is never paired (and {@link evaluateTurns} fails it).
 *
 * TEMPORARY (step B: split on speaker change), plan {@link ABSORB_PLAN} step 5: a final with
 * verdict `caller` whose product span (`user_speaking_start` to `user_speaking_end` of its
 * `utteranceId`, else the final) strictly covers a caller turn's scene start that no utterance
 * claimed absorbs that turn (`absorbed`, turn index to the covering start). There is no VAD pause
 * in the babble and no re-check after a first `caller` verdict. An `other` final never absorbs.
 *
 * The bystander turn then has no utterance; the caller turn index goes into `merged`. Anything
 * else keeps its slot pairing, so a re-checked `caller` that never reaches a caller turn still
 * fails as a bystander barge-in.
 */
export function pairTurnUtterances(
  turns: Turn[],
  seen: Seen[],
): { starts: (Seen | undefined)[]; merged: Set<number>; absorbed: Map<number, Seen> } {
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
    if (final?.speaker !== 'caller' || final.speakerRechecked !== true) return
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
  // TEMPORARY (step B: split on speaker change) - see ABSORB_PLAN, step 5.
  // The cap applies per pass (stricter than per run).
  const absorbed = new Map<number, Seen>()
  turns.forEach((turn, j) => {
    if (turn.role !== 'main' || starts[j] != null) return
    const cover = allStarts.find((s) => {
      if (s.atMs >= turn.startMs) return false
      const final = seen.find(
        (e) => e.type === SPEECH_EVENT_TYPE.userSpeechFinal && e.utteranceId === s.utteranceId,
      )
      if (final?.speaker !== 'caller') return false
      const endMs = utteranceEndMs(s.utteranceId!, seen)
      return endMs != null && endMs > turn.startMs
    })
    if (cover != null) absorbed.set(j, cover)
  })
  return { starts, merged, absorbed }
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

/** Pure evaluation of one pass; exported for the evaluator unit test. */
export function evaluateTurns(turns: Turn[], seen: Seen[], label: string): string[] {
  const failures: string[] = []
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
  const { starts, merged, absorbed } = pairTurnUtterances(turns, seen)
  let absorbedCount = 0
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
      // TEMPORARY (step B: split on speaker change) - see ABSORB_PLAN, step 5.
      const cover = absorbed.get(i)
      const coverId = cover?.utteranceId
      const coverFinal = coverId != null ? finals.get(coverId) : undefined
      if (cover == null || coverId == null || coverFinal == null) {
        failures.push(`${tag}: caller speech start not detected`)
        return
      }
      absorbedCount += 1
      console.log(
        `${tag}: absorbed into utterance ${coverId} (started ${cover.atMs} ms, final caller covers the scene start ${turn.startMs} ms)`,
      )
      if (
        agentSpeakingAt(turn.startMs, seen) &&
        !bargesOf(coverId).some((b) => b.atMs >= turn.startMs)
      ) {
        failures.push(
          `${tag}: absorbed into ${coverId}, agent was speaking at the scene start ${turn.startMs} ms, no barge_in after it`,
        )
      }
      if (turn.kind === 'qualified') {
        const sim = callerWindowSimilarity(turn.text, coverFinal.text ?? '')
        console.log(
          `${tag}: recognized "${coverFinal.text ?? ''}" similarity ${sim.toFixed(2)} (absorbed, best caller-length window)`,
        )
        if (sim < DEFAULT_BARGE_PHRASE_MIN_SIMILARITY) {
          failures.push(
            `${tag}: absorbed into ${coverId}, recognized "${coverFinal.text ?? ''}" similarity ${sim.toFixed(2)}`,
          )
        }
      }
      return
    }
    const isMerged = merged.has(i)
    if (isMerged) {
      console.log(
        `${tag}: merged utterance ${id} (started ${start.atMs} ms, before the caller at ${turn.startMs} ms; final re-checked caller)`,
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
      } else if (isRechecked(start, seen)) {
        // The gate counts every frame while its VAD segment is open (speech, babble, pauses under minSilenceDurationMs; not the pre-roll) and restarts at the last window boundary when a segment rejoins the key after SpeechEnd. Voiced scene time is therefore usually at most the counted speech, so this check understates lateness; it can overstate it only after a rejoin. The speech-end and final checks below bound a barge after the audio stops.
        const windowEnd = verdictAfterMs(start, seen)!
        const late = voicedMsBetween(turns, start.atMs, barge.atMs) - windowEnd
        if (late > BARGE_SLACK_MS) {
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
      // A barge-in released by the deciding window cannot come before that window ends.
      const decided = isRechecked(start, seen) ? verdictAfterMs(start, seen) : undefined
      if (decided != null && delay < decided - BARGE_EARLY_SLACK_MS) {
        failures.push(
          `${tag}: barge-in ${delay} ms after speech start, before the deciding window ends at ${decided} ms (slack ${BARGE_EARLY_SLACK_MS} ms)`,
        )
      }
      // The VAD can open on babble or a bystander tail before the caller speaks, so speech start is
      // not the caller's onset; the cap bounds the caller-perceived latency.
      const capFrom = Math.max(start.atMs, turn.startMs)
      if (barge.atMs - capFrom > BARGE_ABS_CAP_MS && !isRechecked(start, seen)) {
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
      console.log(
        `${tag}: recognized "${final.text ?? ''}" similarity ${sim.toFixed(2)}${isMerged ? ' (best caller-length window)' : ''}`,
      )
      if (sim < DEFAULT_BARGE_PHRASE_MIN_SIMILARITY) {
        failures.push(`${tag}: recognized "${final.text ?? ''}" similarity ${sim.toFixed(2)}`)
      }
    }
  })
  const callerTurns = turns.filter((t) => t.role === 'main').length
  if (absorbedCount > 0) {
    console.log(
      `${label}: caller turns absorbed into an earlier utterance: ${absorbedCount}/${callerTurns}`,
    )
  }
  // TEMPORARY (step B: split on speaker change) - see ABSORB_PLAN, step 5.
  if (callerTurns > 0 && absorbedCount / callerTurns > MAX_ABSORBED_SHARE) {
    failures.push(
      `${label}: ${absorbedCount} of ${callerTurns} caller turns absorbed into an earlier utterance, more than ${MAX_ABSORBED_SHARE * 100} %`,
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
