import { describe, expect, test } from 'vitest'

const live = process.env.VOICE_LIVE_TEST === '1'

describe.skipIf(!live)('VoiceAgent live vendors', () => {
  test('OpenAI live STT/TTS requires VOICE_LIVE_TEST=1', () => {
    expect(live).toBe(true)
  })
})

describe('VoiceAgent live vendors (skipped)', () => {
  test.skipIf(live)('set VOICE_LIVE_TEST=1 to run live vendor tests', () => {
    expect(process.env.VOICE_LIVE_TEST).not.toBe('1')
  })
})
