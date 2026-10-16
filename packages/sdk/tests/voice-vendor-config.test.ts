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

  test('fishaudio-stt preset gates STT on VAD', () => {
    const cfg = voiceConfigForVendor('fishaudio-stt')
    expect(cfg.stt.provider).toBe('fishaudio')
    expect(cfg.stt.model).toBe('transcribe-1')
    expect('vad' in cfg && cfg.vad.gateStt).toBe(true)
  })

  test('lists eleven supported live vendor presets', () => {
    expect(LIVE_VENDOR_METAS).toHaveLength(11)
    expect(LIVE_VENDOR_METAS.map((m) => m.id).sort()).toEqual(
      [
        'assemblyai',
        'aws',
        'azure',
        'cartesia',
        'deepgram',
        'elevenlabs',
        'fishaudio',
        'fishaudio-stt',
        'google',
        'groq',
        'openai',
      ].sort(),
    )
  })
})
