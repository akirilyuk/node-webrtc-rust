import { describe, expect, test } from 'vitest'

import { VoiceAgent } from '../src/voice'
import { LIVE_VENDOR_METAS, type LiveVendorId, voiceConfigForVendor } from './voice-vendor-presets'

describe('VoiceAgent vendor config surface', () => {
  test.each(LIVE_VENDOR_METAS.map((m) => [m.id, m.sttProvider, m.ttsProvider] as const))(
    'constructs VoiceAgent for %s (stt=%s tts=%s)',
    (id, sttProvider, ttsProvider) => {
      const cfg = voiceConfigForVendor(id as LiveVendorId)
      const agent = new VoiceAgent({
        ...cfg,
        events: { mode: 'both' },
      })
      expect(agent).toBeDefined()
      expect(cfg.stt.provider).toBe(sttProvider)
      expect(cfg.tts.provider).toBe(ttsProvider)
    },
  )

  test('lists nine supported live vendor presets', () => {
    expect(LIVE_VENDOR_METAS).toHaveLength(9)
    expect(LIVE_VENDOR_METAS.map((m) => m.id).sort()).toEqual(
      [
        'assemblyai',
        'aws',
        'azure',
        'cartesia',
        'deepgram',
        'elevenlabs',
        'google',
        'groq',
        'openai',
      ].sort(),
    )
  })
})
