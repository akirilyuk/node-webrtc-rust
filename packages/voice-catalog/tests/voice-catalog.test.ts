import { describe, expect, test } from 'vitest'
import { readFileSync } from 'node:fs'
import {
  VOICE_VENDORS,
  allVendorIds,
  azureTtsVoices,
  customerSttProviderIds,
  customerTtsProviderIds,
  getVendor,
  sttModelAllowlist,
  ttsModelAllowlist,
  ttsVoiceAllowlist,
  validateVoiceCatalogChoice,
} from '../src/index.js'

const EXPECTED_VENDOR_IDS = [
  'openai',
  'deepgram',
  'elevenlabs',
  'cartesia',
  'assemblyai',
  'google',
  'groq',
  'azure',
  'aws',
  'local-sherpa',
  'mock',
] as const

describe('voice catalog vendors', () => {
  test('includes every legacy VoiceVendorId', () => {
    expect(allVendorIds().sort()).toEqual([...EXPECTED_VENDOR_IDS].sort())
  })

  test('customer provider lists exclude mock', () => {
    expect(customerSttProviderIds()).not.toContain('mock')
    expect(customerTtsProviderIds()).not.toContain('mock')
    expect(customerSttProviderIds().length).toBeGreaterThan(0)
    expect(customerTtsProviderIds().length).toBeGreaterThan(0)
  })
})

describe('groq allowlists (crate vendor-groq matrix)', () => {
  test('STT models exact', () => {
    expect(sttModelAllowlist('groq')).toEqual(['whisper-large-v3-turbo', 'whisper-large-v3'])
  })

  test('TTS models exact', () => {
    expect(ttsModelAllowlist('groq')).toEqual([
      'canopylabs/orpheus-v1-english',
      'canopylabs/orpheus-arabic-saudi',
    ])
  })
})

describe('azure TTS voices snapshot', () => {
  test('761 voices including documented ShortNames', () => {
    expect(azureTtsVoices.length).toBe(761)
    expect(azureTtsVoices).toContain('en-US-JennyNeural')
    expect(azureTtsVoices).toContain('en-US-Ava:DragonHDLatestNeural')
  })

  test('vendor entry points at voice list file', () => {
    const azure = getVendor('azure')
    expect(azure?.ttsVoiceListFile).toBe('azure-tts-voices.json')
    expect(azure?.defaultTtsVoice).toBe('en-US-JennyNeural')
    expect(ttsVoiceAllowlist('azure').length).toBe(761)
  })
})

describe('validateVoiceCatalogChoice', () => {
  test('rejects unknown STT model for groq', () => {
    expect(validateVoiceCatalogChoice('groq', 'sttModel', 'whisper-1')).toMatchObject({
      kind: 'sttModel',
      vendorId: 'groq',
    })
  })

  test('accepts documented groq STT model', () => {
    expect(validateVoiceCatalogChoice('groq', 'sttModel', 'whisper-large-v3-turbo')).toBeNull()
  })

  test('rejects unknown azure TTS voice', () => {
    expect(validateVoiceCatalogChoice('azure', 'ttsVoice', 'not-a-real-voice')).toMatchObject({
      kind: 'ttsVoice',
      vendorId: 'azure',
    })
  })
})

describe('crate-aligned allowlist snapshots', () => {
  test('openai STT models', () => {
    expect(sttModelAllowlist('openai')).toEqual([
      'whisper-1',
      'gpt-4o-mini-transcribe',
      'gpt-4o-transcribe',
      'gpt-4o-transcribe-diarize',
      'gpt-transcribe',
      'gpt-live-transcribe',
    ])
  })

  test('openai TTS models', () => {
    expect(ttsModelAllowlist('openai')).toEqual([
      'tts-1',
      'tts-1-hd',
      'gpt-4o-mini-tts',
      'gpt-4o-mini-tts-2025-12-15',
    ])
  })

  test('deepgram listen models', () => {
    expect(sttModelAllowlist('deepgram')).toEqual(['nova-2', 'nova-3'])
  })

  test('assemblyai speech models', () => {
    expect(sttModelAllowlist('assemblyai')).toEqual([
      'universal-3-6-pro',
      'universal-streaming-english',
    ])
  })

  test('cartesia websocket models', () => {
    expect(ttsModelAllowlist('cartesia')).toEqual([
      'sonic-3.6',
      'sonic-3.5',
      'sonic-3',
      'sonic-latest',
    ])
  })

  test('google STT models', () => {
    expect(sttModelAllowlist('google')).toEqual(['chirp_3', 'chirp_2', 'telephony', 'latest_long'])
  })

  test('aws STT language codes and Polly voices', () => {
    expect(sttModelAllowlist('aws')).toEqual([
      'en-US',
      'es-US',
      'fr-FR',
      'de-DE',
      'pt-BR',
      'ja-JP',
      'ko-KR',
      'zh-CN',
      'it-IT',
      'hi-IN',
    ])
    expect(ttsVoiceAllowlist('aws')).toEqual([
      'Joanna',
      'Matthew',
      'Amy',
      'Brian',
      'Ruth',
      'Stephen',
    ])
  })

  test('azure STT recognition modes', () => {
    expect(sttModelAllowlist('azure')).toEqual(['conversation'])
  })

  test('elevenlabs STT model', () => {
    expect(sttModelAllowlist('elevenlabs')).toEqual(['scribe_v2_realtime'])
  })

  test('every vendor has home URL', () => {
    for (const vendor of VOICE_VENDORS) {
      expect(vendor.home).toMatch(/^https?:\/\//)
    }
  })
})

describe('esm json import attributes', () => {
  test('postbuild patch script rewrites both catalog JSON imports', () => {
    const src = readFileSync(
      new URL('../scripts/patch-esm-json-imports.mjs', import.meta.url),
      'utf8',
    )
    expect(src).toContain("from '../catalog/vendors.json' with { type: 'json' }")
    expect(src).toContain("from '../catalog/azure-tts-voices.json' with { type: 'json' }")
  })
})
