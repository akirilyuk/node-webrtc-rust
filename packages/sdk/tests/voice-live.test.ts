import { describe, expect, test } from 'vitest'

import { VoiceAgent } from '../src/voice'
import { createVoiceLoopback } from './voice-helpers'
import {
  LIVE_VENDOR_METAS,
  type LiveVendorId,
  liveVendorEnabled,
  voiceConfigForVendor,
} from './voice-vendor-presets'

const LIVE_TIMEOUT_MS = 120_000

describe('VoiceAgent live vendors (opt-in)', () => {
  for (const meta of LIVE_VENDOR_METAS) {
    describe.skipIf(!liveVendorEnabled(meta.id))(`${meta.id} live smoke`, () => {
      test(
        'TTS injection on loopback PC',
        async () => {
          const { agentOut, userInbound, cleanup } = await createVoiceLoopback()
          const cfg = voiceConfigForVendor(meta.id)

          const agent = new VoiceAgent({
            ...cfg,
            events: { mode: 'both' },
          })

          await agent.attach({ inboundTrack: userInbound, outboundTrack: agentOut })
          await agent.start()

          await agent.sendTextToTTS(`Live ${meta.id} vendor smoke test.`)

          await agent.stop()
          await cleanup()
        },
        LIVE_TIMEOUT_MS,
      )
    })

    describe.skipIf(liveVendorEnabled(meta.id))(`${meta.id} skipped without credentials`, () => {
      test('documents how to enable', () => {
        expect(liveVendorEnabled(meta.id)).toBe(false)
      })
    })
  }
})

describe('VoiceAgent live vendor gate', () => {
  test('requires VOICE_LIVE_TEST=1 and per-vendor flag', () => {
    const id: LiveVendorId = 'openai'
    const prevGlobal = process.env.VOICE_LIVE_TEST
    const prevVendor = process.env.VOICE_LIVE_OPENAI
    delete process.env.VOICE_LIVE_TEST
    delete process.env.VOICE_LIVE_OPENAI
    expect(liveVendorEnabled(id)).toBe(false)
    process.env.VOICE_LIVE_TEST = prevGlobal
    process.env.VOICE_LIVE_OPENAI = prevVendor
  })
})
