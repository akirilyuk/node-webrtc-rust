/**
 * Pure helpers of `roundtrip-noisy-bystander-nopause.ts` (no models needed; Vitest in
 * `roundtrip-noisy-bystander-nopause.test.ts`).
 *
 * The no-pause variant plays `other_to_caller` scenes (`speaker_fixtures --other-to-caller`,
 * `s4_test_nNN`): a caller enrollment sentence, a loud bystander, then the caller 0-250 ms after
 * the bystander's last voiced frame, over a babble bed that keeps the adaptive VAD open. With
 * `speakerGate.splitOnSpeakerChange` the agent must cut the bystander's utterance where the caller
 * starts (`user_utterance_split` with `from: 'other'`, `to: 'caller'`), so the caller's final holds
 * only the caller's words.
 *
 * Oracle (plan 2026-11-13-speaker-split-other-to-caller, decisions h and l):
 * - every pairing goes by id (`utteranceId`, `fromUtteranceId`/`toUtteranceId`), never by
 *   adjacency or arrival order (rule oracle-relies-only-on-promised-order);
 * - a `caller` final with bystander words fails (cut-point leak), and so does any leading glue
 *   (a word before the caller phrase that ends the bystander sentence). There is no "credited to
 *   the caller turn" exception;
 * - a leak caused by a gate false accept (the other→caller split lies so far before the caller's
 *   voiced onset that no window holding caller speech can have confirmed it) is reported as its
 *   own class and does not fail the pass;
 * - scenes whose bystander voice the gate cannot separate from the caller (pair FAR >= 5 % in
 *   `crates/vendor-sherpa-onnx/examples/FALSE_ACCEPT_BENCH.md`) are not played.
 */

import { SPEECH_EVENT_TYPE } from '@node-webrtc-rust/sdk/voice'

import { DEFAULT_BARGE_PHRASE_MIN_SIMILARITY } from './roundtrip-barge-in-helpers.js'
import { normalizeForCompare, wordSimilarity } from './roundtrip-counting.js'
import {
  CHECK_SPEECH_MS,
  callerWindowSimilarity,
  splitSceneMs,
  type Seen,
} from './roundtrip-noisy-bystander-helpers.js'

/** The scenes' caller voice, as `FALSE_ACCEPT_BENCH.md` names it. */
export const CALLER_BENCH_LABEL = 'amy-low'
/** A scene is not played when its bystander's pair FAR against the caller reaches this (bed or clean). */
export const PAIR_FAR_EXCLUDE_PCT = 5
/**
 * Bystander voices excluded from the no-pause scenes: every `amy-low ← X` pair at or above
 * {@link PAIR_FAR_EXCLUDE_PCT} in the bench's `titanet 1.5s raw @0.459` column (today's gate).
 * Lockstep test: `roundtrip-noisy-bystander-nopause.test.ts` parses the bench.
 */
export const EXCLUDED_BYSTANDER_VOICES: readonly { voice: string; sid: number }[] = [
  { voice: 'en_US-libritts_r-medium', sid: 120 },
]
/** Leading glue: a word before the caller phrase equal to one of the bystander's last this many words. */
export const LEADING_GLUE_TAIL_WORDS = 3
/** Shortest word that ties a caller final to the bystander sentence (drops "the", "and", "a"). */
export const LEAK_MIN_WORD_LENGTH = 4

export interface NoPauseSegment {
  role: 'main' | 'bystander'
  startMs: number
  endMs: number
  /** First voiced 20 ms frame of the clip (manifest `voiced_start_ms`). */
  voicedStartMs?: number
  voicedEndMs?: number
  text: string
  kind: 'qualified' | 'long' | 'background'
  enroll?: boolean
  voice?: string
  sid?: number
}

/** Voice name as the bench tables write it: `en_US-libritts_r-medium` sid 120 -> `libritts_r 120`. */
export function benchVoiceLabel(voice: string, sid: number): string {
  const base = voice.replace(/^en_US-/, '').replace(/-(low|medium|high)$/, '')
  if (base === 'amy') return 'amy-low'
  return base === 'libritts_r' ? `${base} ${sid}` : base
}

/**
 * `caller ← other` FAR (bed / clean %) for one caller from the bench's "FAR per pair" table, first
 * score column (`titanet 1.5s raw @0.459`, today's gate). Keyed by the other voice's bench label.
 */
export function parseCallerPairFar(
  md: string,
  caller: string = CALLER_BENCH_LABEL,
): Map<string, { bed: number; clean: number }> {
  const out = new Map<string, { bed: number; clean: number }>()
  const start = md.indexOf('### FAR per pair (caller ← other)')
  if (start < 0) return out
  const lines = md.slice(start).split('\n').slice(1)
  let header: string[] | undefined
  for (const line of lines) {
    if (line.startsWith('#')) break
    if (!line.startsWith('|')) continue
    const cells = line
      .split('|')
      .slice(1, -1)
      .map((c) => c.trim())
    if (header == null) {
      header = cells
      continue
    }
    if (cells[0]?.startsWith('---')) continue
    if (header[1] !== 'titanet 1.5s raw @0.459') return new Map()
    const m = /^(.+?) ← (.+)$/.exec(cells[0] ?? '')
    if (m == null || m[1] !== caller) continue
    const v = /^([\d.]+) \/ ([\d.]+)$/.exec(cells[1] ?? '')
    if (v == null) continue
    out.set(m[2]!, { bed: Number(v[1]), clean: Number(v[2]) })
  }
  return out
}

/** Bystander voices whose pair FAR against the caller reaches {@link PAIR_FAR_EXCLUDE_PCT}. */
export function inseparableBystanderLabels(md: string): string[] {
  return [...parseCallerPairFar(md).entries()]
    .filter(([, far]) => Math.max(far.bed, far.clean) >= PAIR_FAR_EXCLUDE_PCT)
    .map(([label]) => label)
    .sort()
}

export function isExcludedBystander(voice: string | undefined, sid: number | undefined): boolean {
  return EXCLUDED_BYSTANDER_VOICES.some((v) => v.voice === voice && v.sid === (sid ?? 0))
}

function words(text: string): string[] {
  return normalizeForCompare(text)
    .split(' ')
    .filter((w) => w.length > 0)
}

export interface NoPauseLeak {
  utteranceId: string
  text: string
  /** Bystander-only words found in the caller final. */
  words: string[]
  /** Split point on the scene clock (other→caller split that opened this utterance), if any. */
  sceneSplitMs?: number
}

export interface NoPauseSceneResult {
  failures: string[]
  /** `user_utterance_split` events with `from: 'other'` and `to: 'caller'`. */
  otherToCallerSplits: number
  /** Caller finals with bystander words after a cut that a caller window confirmed (each fails). */
  cutPointLeaks: NoPauseLeak[]
  /** Caller finals with a word before the caller phrase from the bystander's last words (each fails). */
  leadingGlue: NoPauseLeak[]
  /** Leaks after a gate false accept (reported, not failing). */
  gateFalseAcceptLeaks: NoPauseLeak[]
}

/**
 * One no-pause scene, one fresh agent. `seen` holds that agent's events only; `sceneStartMediaMs`
 * is its inbound media clock when the scene started (for `fromStartMs + splitAtMs`).
 */
export function evaluateNoPauseScene(
  segments: NoPauseSegment[],
  seen: Seen[],
  label: string,
  sceneStartMediaMs: number,
): NoPauseSceneResult {
  const failures: string[] = []
  const cutPointLeaks: NoPauseLeak[] = []
  const leadingGlue: NoPauseLeak[] = []
  const gateFalseAcceptLeaks: NoPauseLeak[] = []
  const enroll = segments.find((s) => s.role === 'main' && s.enroll === true)
  const bystander = segments.find((s) => s.role === 'bystander')
  const caller = segments.find((s) => s.role === 'main' && s.enroll !== true)
  if (enroll == null || bystander == null || caller == null) {
    return {
      failures: [`${label}: scene needs an enrollment, a bystander and a caller segment`],
      otherToCallerSplits: 0,
      cutPointLeaks,
      leadingGlue,
      gateFalseAcceptLeaks,
    }
  }

  // Last final per utterance id (a final is emitted once per id; last wins if not).
  const finals = new Map<string, Seen>()
  for (const e of seen) {
    if (e.type === SPEECH_EVENT_TYPE.userSpeechFinal && e.utteranceId != null) {
      finals.set(e.utteranceId, e)
    }
  }
  const barges = seen.filter((e) => e.type === SPEECH_EVENT_TYPE.bargeIn)
  for (const b of barges) {
    if (b.utteranceId == null) {
      failures.push(`${label}: barge_in at ${b.atMs} ms without utteranceId`)
    } else if (finals.get(b.utteranceId)?.speaker === 'other') {
      failures.push(
        `${label}: barge_in at ${b.atMs} ms for utterance ${b.utteranceId} whose final is speaker other`,
      )
    }
  }
  const splits = seen.filter((e) => e.type === SPEECH_EVENT_TYPE.userUtteranceSplit)
  const cutIds = new Set(splits.map((s) => s.fromUtteranceId).filter((id) => id != null))
  const otherToCaller = splits.filter((s) => s.from === 'other' && s.to === 'caller')
  // The utterance that ends at the split must say other: it is the bystander's part.
  for (const s of otherToCaller) {
    const f = s.fromUtteranceId != null ? finals.get(s.fromUtteranceId) : undefined
    if (f != null && f.speaker !== 'other') {
      failures.push(
        `${label}: other→caller split cut ${String(s.fromUtteranceId)}, whose final is speaker ${String(f.speaker)}`,
      )
    }
  }

  // The caller's utterance, by id: the to-utterance of an other→caller split that was not cut
  // again. Without such a split (the VAD closed in the 0-250 ms gap), the one caller final that is
  // neither the enrollment utterance (first speech start) nor cut.
  const enrollId = seen.find(
    (e) => e.type === SPEECH_EVENT_TYPE.userSpeakingStart && e.utteranceId != null,
  )?.utteranceId
  // The caller part is the last other→caller to-utterance on its split chain: it may be cut again
  // caller→other (bed or babble after the caller's last word) and still be the caller's utterance,
  // but not when a later other→caller split on the same chain follows.
  const leadsToOtherToCaller = (id: string): boolean => {
    const seenIds = new Set<string>()
    let cur: string | undefined = id
    while (cur != null && !seenIds.has(cur)) {
      seenIds.add(cur)
      const next = splits.find((x) => x.fromUtteranceId === cur)
      if (next == null) return false
      if (next.from === 'other' && next.to === 'caller') return true
      cur = next.toUtteranceId
    }
    return false
  }
  const ends = otherToCaller.filter(
    (s) => s.toUtteranceId != null && !leadsToOtherToCaller(s.toUtteranceId),
  )
  let callerId: string | undefined
  let callerSplit: Seen | undefined
  if (ends.length > 1) {
    failures.push(
      `${label}: ${ends.length} caller utterances after the bystander (${ends.map((s) => s.toUtteranceId).join(', ')}), expected one`,
    )
  } else if (ends.length === 1) {
    callerSplit = ends[0]
    callerId = callerSplit!.toUtteranceId
  } else {
    const own = [...finals.values()].filter(
      (f) => f.speaker === 'caller' && f.utteranceId !== enrollId && !cutIds.has(f.utteranceId!),
    )
    if (own.length === 1) {
      callerId = own[0]!.utteranceId
      console.log(`${label}: no other→caller split; caller utterance ${callerId} opened on its own`)
    }
  }
  const tag = `${label} caller "${caller.text.slice(0, 30)}"`
  const callerFinal = callerId != null ? finals.get(callerId) : undefined
  if (callerId == null) {
    failures.push(`${tag}: no caller utterance of its own (no other→caller split)`)
  } else if (callerFinal == null) {
    failures.push(`${tag}: no user_speech_final for caller utterance ${callerId}`)
  } else {
    if (callerFinal.speaker !== 'caller') {
      failures.push(`${tag}: final speaker ${String(callerFinal.speaker)}, expected caller`)
    }
    if (!barges.some((b) => b.utteranceId === callerId)) {
      failures.push(`${tag}: no barge_in on caller utterance ${callerId}`)
    }
    if (caller.kind === 'qualified') {
      const sim = callerWindowSimilarity(caller.text, callerFinal.text ?? '')
      console.log(`${tag}: recognized "${callerFinal.text ?? ''}" similarity ${sim.toFixed(2)}`)
      if (sim < DEFAULT_BARGE_PHRASE_MIN_SIMILARITY) {
        failures.push(`${tag}: recognized "${callerFinal.text ?? ''}" similarity ${sim.toFixed(2)}`)
      }
    } else {
      console.log(`${tag}: recognized "${callerFinal.text ?? ''}" (long sentence, not judged)`)
    }
  }

  const callerOnsetMs = caller.voicedStartMs ?? caller.startMs
  const sceneSplitOf = (s: Seen | undefined): number | undefined =>
    s != null ? splitSceneMs(s, sceneStartMediaMs) : undefined
  // Gate false accept (decision l): the window that decided the other→caller split ended at or
  // before the caller's voiced onset, so it scored bystander audio only. The n-th check window
  // needs n × checkSpeechMs of speech (types.ts `speakerCheckWindows`), so the deciding window ends
  // no earlier than U1 `fromStartMs` + k × checkSpeechMs on the media clock, where k is the cut
  // utterance's (U1's) final `speakerCheckWindows`. That count includes the deciding window: in
  // s4_test_n02 (.test-logs/20261113-224109-nopause-n02-debug.log, re-run with final windows logged
  // in .test-logs/20261113-224730-nopause-n02-windows.log) the split utt-2 -> utt-4 followed the
  // `re-check key=2 window=3` score 0.470, and utt-2's final carried speakerCheckWindows=3. A
  // missing field or split time is a cut-point leak (fail closed).
  const decidingWindowEndSceneMs = (s: Seen | undefined): number | undefined => {
    if (s?.fromStartMs == null || s.fromUtteranceId == null) return undefined
    const k = finals.get(s.fromUtteranceId)?.speakerCheckWindows
    if (k == null) return undefined
    return s.fromStartMs + k * CHECK_SPEECH_MS - sceneStartMediaMs
  }
  const isFalseAccept = (
    id: string,
  ): { yes: boolean; sceneSplitMs?: number; windowEndMs?: number } => {
    const s = otherToCaller.find((x) => x.toUtteranceId === id)
    const end = decidingWindowEndSceneMs(s)
    return {
      yes: end != null && end <= callerOnsetMs,
      sceneSplitMs: sceneSplitOf(s),
      windowEndMs: end,
    }
  }

  // Bystander words: long words of the bystander sentence that no caller sentence has.
  const callerWords = new Set([...words(enroll.text), ...words(caller.text)])
  const bystanderOnly = new Set(
    words(bystander.text).filter((w) => w.length >= LEAK_MIN_WORD_LENGTH && !callerWords.has(w)),
  )
  for (const f of finals.values()) {
    if (f.speaker !== 'caller' || f.utteranceId == null) continue
    const leaked = [...new Set(words(f.text ?? ''))].filter((w) => bystanderOnly.has(w))
    if (leaked.length === 0) continue
    const fa = isFalseAccept(f.utteranceId)
    const leak: NoPauseLeak = {
      utteranceId: f.utteranceId,
      text: f.text ?? '',
      words: leaked,
      sceneSplitMs: fa.sceneSplitMs,
    }
    if (fa.yes) {
      gateFalseAcceptLeaks.push(leak)
      console.log(
        `${label}: gate false-accept leak in ${f.utteranceId} (split at ${String(fa.sceneSplitMs)} ms, deciding window ends ${String(fa.windowEndMs)} ms, caller onset ${callerOnsetMs} ms): ${leaked.join(' ')}: "${leak.text}"`,
      )
    } else {
      cutPointLeaks.push(leak)
      failures.push(
        `${label}: caller final ${f.utteranceId} holds bystander words (${leaked.join(' ')}), split at ${String(fa.sceneSplitMs ?? 'n/a')} ms, deciding window ends ${String(fa.windowEndMs ?? 'n/a (no speakerCheckWindows)')} ms, caller onset ${callerOnsetMs} ms: "${leak.text}"`,
      )
    }
  }

  // Leading glue on the caller's own final: a word before its best caller-length window that is
  // one of the bystander's last words.
  if (callerId != null && callerFinal?.speaker === 'caller' && !isFalseAccept(callerId).yes) {
    const cw = words(caller.text)
    const fw = words(callerFinal.text ?? '')
    const n = cw.length
    if (n > 0 && fw.length > n) {
      let bestK = 0
      let best = -1
      for (let k = 0; k + n <= fw.length; k += 1) {
        const sim = wordSimilarity(caller.text, fw.slice(k, k + n).join(' '))
        if (sim > best) {
          best = sim
          bestK = k
        }
      }
      const tail = words(bystander.text).slice(-LEADING_GLUE_TAIL_WORDS)
      const glued = fw.slice(0, bestK).filter((w) => tail.includes(w))
      if (glued.length > 0) {
        leadingGlue.push({
          utteranceId: callerId,
          text: callerFinal.text ?? '',
          words: glued,
          sceneSplitMs: sceneSplitOf(callerSplit),
        })
        failures.push(
          `${tag}: leading glue (${glued.join(' ')}) from the bystander's last words in ${callerId}: "${callerFinal.text ?? ''}"`,
        )
      }
    }
  }

  for (const f of finals.values()) {
    console.log(
      `${label}: final ${String(f.utteranceId)} speaker=${String(f.speaker)} rechecked=${String(f.speakerRechecked)} speakerCheckWindows=${String(f.speakerCheckWindows)}`,
    )
  }
  for (const s of splits) {
    console.log(
      `${label}: user_utterance_split ${String(s.from)}→${String(s.to)} ${String(s.fromUtteranceId)} -> ${String(s.toUtteranceId)} sceneSplitMs=${String(sceneSplitOf(s) ?? 'n/a')} callerOnsetMs=${callerOnsetMs} bystanderVoicedEndMs=${String(bystander.voicedEndMs ?? bystander.endMs)}`,
    )
  }
  return {
    failures,
    otherToCallerSplits: otherToCaller.length,
    cutPointLeaks,
    leadingGlue,
    gateFalseAcceptLeaks,
  }
}

/** Silence the harness inserts after the enrollment sentence, so that utterance ends and enrolls. */
export const ENROLL_PAUSE_MS = 2500
/** The pause starts this long after the enrollment segment's end (its clip tail). */
export const ENROLL_PAUSE_AFTER_MS = 200

/**
 * The gate enrolls the caller only after a final (`SpeakerGate::enroll_utterance`), and the S4 bed
 * keeps the VAD open from the enrollment sentence to the end of the scene. The harness therefore
 * plays the scene up to `insertAtMs`, then `insertMs` of silence, then the rest. Segments that start
 * at or after `insertAtMs` move by exactly `insertMs`; earlier ones stay.
 */
export function shiftSegmentsAfter(
  segments: NoPauseSegment[],
  insertAtMs: number,
  insertMs: number,
): NoPauseSegment[] {
  const shift = (v: number | undefined): number | undefined => (v == null ? v : v + insertMs)
  return segments.map((s) =>
    s.startMs >= insertAtMs
      ? {
          ...s,
          startMs: s.startMs + insertMs,
          endMs: s.endMs + insertMs,
          voicedStartMs: shift(s.voicedStartMs),
          voicedEndMs: shift(s.voicedEndMs),
        }
      : s,
  )
}

/** Where the enrollment pause goes: the enrollment segment's end + {@link ENROLL_PAUSE_AFTER_MS}. */
export function enrollPauseAtMs(segments: NoPauseSegment[]): number | undefined {
  const enroll = segments.find((s) => s.role === 'main' && s.enroll === true)
  return enroll != null ? enroll.endMs + ENROLL_PAUSE_AFTER_MS : undefined
}

/**
 * `SHERPA_NOPAUSE_ONLY` (comma list of scene ids, for debugging one scene): the ids to play, or
 * `undefined` (play every scene) when unset or blank.
 */
export function parseSceneFilter(raw: string | undefined): Set<string> | undefined {
  const ids = (raw ?? '')
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0)
  return ids.length > 0 ? new Set(ids) : undefined
}

export function sceneSelected(id: string, filter: Set<string> | undefined): boolean {
  return filter == null || filter.has(id)
}
