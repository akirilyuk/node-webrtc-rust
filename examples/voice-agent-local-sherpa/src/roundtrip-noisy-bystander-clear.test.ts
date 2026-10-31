import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import {
  SPEAKER_GATE_RECHECK_MAX_WINDOWS,
  VOICE_AGENT_VAD_PRESET,
} from '@node-webrtc-rust/sdk/voice'
import { describe, expect, it } from 'vitest'

import { CHECK_SPEECH_MS } from './roundtrip-noisy-bystander-helpers.js'

/**
 * Lockstep with the fixture arguments in `scripts/ci/ensure-speaker-fixtures.sh` (that script's
 * comment cites this test). Plan:
 * `development/node-webrtc-rust/plans/2026-10-31-speaker-gate-continuous-recheck.md` (chunk 3).
 *
 * The scene keeps babble clear before a caller turn for LESS than the VAD end silence on purpose:
 * babble and bystander then merge into caller utterances, the case the speaker gate's further
 * windows (`recheckMaxWindows`) must catch. It also has to stay inside the product promise: the
 * longest bystander lead into a caller turn must fit in the windows the gate scores after the
 * first two.
 */
const code = readFileSync(
  join(
    dirname(fileURLToPath(import.meta.url)),
    '..',
    '..',
    '..',
    'scripts',
    'ci',
    'ensure-speaker-fixtures.sh',
  ),
  'utf8',
)
  .split('\n')
  .filter((l) => !l.trimStart().startsWith('#'))
  .join('\n')

const arg = (name: string): number => {
  const m = new RegExp(`${name}\\s+(\\d+)`).exec(code)
  expect(m, `ensure-speaker-fixtures.sh passes ${name}`).not.toBeNull()
  return Number(m![1])
}

/** Upper end of a long-pool bystander sentence ("about 4-6 s of Piper speech", speaker_fixtures.rs). */
const LONG_BYSTANDER_MAX_MS = 6000

describe('noisy-bystander scene babble clear window', () => {
  it('is below the VAD preset end silence, so babble and bystanders merge into caller utterances', () => {
    const vad = VOICE_AGENT_VAD_PRESET
    const endSilenceMs = (vad.minSilenceDurationMs ?? 0) + (vad.speechPadMs ?? 0)
    expect(endSilenceMs).toBeGreaterThan(0)
    expect(arg('--babble-clear-ms')).toBeLessThan(endSilenceMs)
  })

  it('keeps the longest bystander lead into a caller turn inside the gate windows after the first two', () => {
    expect(arg('--long-bystanders')).toBeGreaterThan(0)
    const leadMs = LONG_BYSTANDER_MAX_MS + arg('--min-turn-gap-ms')
    expect(leadMs).toBeLessThan((SPEAKER_GATE_RECHECK_MAX_WINDOWS - 2) * CHECK_SPEECH_MS)
  })
})
