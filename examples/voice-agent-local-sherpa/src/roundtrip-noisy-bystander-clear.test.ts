import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { VOICE_AGENT_VAD_PRESET } from '@node-webrtc-rust/sdk/voice'
import { describe, expect, it } from 'vitest'

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

describe('noisy-bystander scene babble clear window', () => {
  it('is below the VAD preset end silence, so babble and bystanders merge into caller utterances', () => {
    const vad = VOICE_AGENT_VAD_PRESET
    const endSilenceMs = (vad.minSilenceDurationMs ?? 0) + (vad.speechPadMs ?? 0)
    expect(endSilenceMs).toBeGreaterThan(0)
    expect(arg('--babble-clear-ms')).toBeLessThan(endSilenceMs)
  })

  it('uses long bystanders (the lead bound is checked on the real manifest at run time)', () => {
    expect(arg('--long-bystanders')).toBeGreaterThan(0)
  })
})
