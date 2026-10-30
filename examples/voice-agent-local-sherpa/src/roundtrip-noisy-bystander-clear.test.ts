import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { VOICE_AGENT_VAD_PRESET } from '@node-webrtc-rust/sdk/voice'
import { describe, expect, it } from 'vitest'

/**
 * Lockstep with `--babble-clear-ms` in `scripts/ci/ensure-speaker-fixtures.sh` (that script's comment
 * cites this test). Babble must stay clear before a turn for longer than the VAD preset needs to end
 * an utterance, or babble keeps the VAD open and the caller turn is stitched onto earlier speech.
 */
const MARGIN_MS = 300

describe('noisy-bystander scene babble clear window', () => {
  it('is at least the VAD preset end silence plus margin', () => {
    const script = join(
      dirname(fileURLToPath(import.meta.url)),
      '..',
      '..',
      '..',
      'scripts',
      'ci',
      'ensure-speaker-fixtures.sh',
    )
    const code = readFileSync(script, 'utf8')
      .split('\n')
      .filter((l) => !l.trimStart().startsWith('#'))
      .join('\n')
    const m = /--babble-clear-ms\s+(\d+)/.exec(code)
    expect(m, 'ensure-speaker-fixtures.sh passes --babble-clear-ms').not.toBeNull()
    const clearMs = Number(m![1])
    const vad = VOICE_AGENT_VAD_PRESET
    const endSilenceMs = (vad.minSilenceDurationMs ?? 0) + (vad.speechPadMs ?? 0)
    expect(endSilenceMs).toBeGreaterThan(0)
    expect(clearMs).toBeGreaterThanOrEqual(endSilenceMs + MARGIN_MS)
  })
})
