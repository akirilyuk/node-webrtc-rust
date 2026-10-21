import { describe, expect, test } from 'vitest'

import { VOICE_AGENT_VAD_PRESET, VoiceAgent } from '../src/voice'
import { toJsSttConfig, toJsTtsConfig } from '../src/voice/VoiceAgent'
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

  test('fishaudio-stt preset gates STT on VAD and barges in on VAD', () => {
    const cfg = voiceConfigForVendor('fishaudio-stt')
    expect(cfg.stt.provider).toBe('fishaudio')
    expect(cfg.stt.model).toBe('transcribe-1')
    expect('vad' in cfg && cfg.vad.gateStt).toBe(true)
    expect('vad' in cfg && cfg.vad.bargeIn.requireSttPartial).toBe(false)
  })

  test('mistral-stt preset gates STT on VAD and keeps the default partial rule', () => {
    const cfg = voiceConfigForVendor('mistral-stt')
    expect(cfg.stt.provider).toBe('mistral')
    expect(cfg.stt.model).toBe('voxtral-mini-transcribe-realtime-3-0')
    expect(cfg.tts.provider).toBe('mistral')
    expect('vad' in cfg && cfg.vad.gateStt).toBe(true)
    expect('vad' in cfg && cfg.vad.bargeIn.requireSttPartial).toBe(
      VOICE_AGENT_VAD_PRESET.bargeIn.requireSttPartial,
    )
  })

  test('mistral TTS and STT configs reach the native config', () => {
    expect(toJsTtsConfig({ provider: 'mistral', model: 'voxtral-mini-tts-3-0' })).toMatchObject({
      provider: 'mistral',
      model: 'voxtral-mini-tts-3-0',
    })
    expect(
      toJsSttConfig({
        provider: 'mistral',
        model: 'voxtral-mini-transcribe-realtime-3',
        language: 'en',
      }),
    ).toMatchObject({
      provider: 'mistral',
      model: 'voxtral-mini-transcribe-realtime-3',
      language: 'en',
    })
  })

  test('mistral rejects a model that is not documented', () => {
    const make = (tts: string, stt: string) =>
      new VoiceAgent({
        stt: { provider: 'mistral', model: stt },
        tts: { provider: 'mistral', model: tts },
      })
    expect(() => make('voxtral-mini-tts-latest', 'voxtral-mini-transcribe-realtime-3-0')).toThrow(
      /unsupported Mistral TTS model/,
    )
    expect(() =>
      make('voxtral-mini-tts-realtime-3-0', 'voxtral-mini-transcribe-realtime-latest'),
    ).toThrow(/unsupported Mistral STT model/)
    expect(() =>
      make('voxtral-mini-tts-realtime-3-0', 'voxtral-mini-transcribe-realtime-3-0'),
    ).not.toThrow()
  })

  test('lists thirteen supported live vendor presets', () => {
    expect(LIVE_VENDOR_METAS).toHaveLength(13)
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
        'mistral',
        'mistral-stt',
        'openai',
      ].sort(),
    )
  })
})
