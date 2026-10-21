import { describe, expect, test } from 'vitest'
import { readFileSync } from 'node:fs'
import {
  VOICE_VENDORS,
  allVendorIds,
  azureTtsVoices,
  customerSttProviderIds,
  customerTtsProviderIds,
  defaultSttTransport,
  defaultTtsTransport,
  getVendor,
  googleTtsVoices,
  sttModelAllowlist,
  sttTransports,
  ttsModelAllowlist,
  ttsTransports,
  ttsVoiceAllowlist,
  validateVoiceCatalogChoice,
} from '../src/index.js'

const EXPECTED_VENDOR_IDS = [
  'openai',
  'deepgram',
  'elevenlabs',
  'cartesia',
  'fishaudio',
  'mistral',
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
  test('mistral models, defaults and open voice ids', () => {
    const vendor = getVendor('mistral')
    expect(vendor?.stt).toBe(true)
    expect(vendor?.tts).toBe(true)
    expect(vendor?.defaultTtsModel).toBe('voxtral-mini-tts-3-0')
    expect(vendor?.defaultTtsVoice).toBe('voice_478')
    expect(vendor?.defaultSttModel).toBe('voxtral-mini-transcribe-realtime-3-0')
    expect(ttsModelAllowlist('mistral')).toEqual([
      'voxtral-mini-tts-3-0',
      'voxtral-mini-tts-3',
      'voxtral-mini-tts-realtime-3-0',
    ])
    expect(ttsModelAllowlist('mistral')).not.toContain('voxtral-mini-tts-latest')
    expect(sttModelAllowlist('mistral')).toEqual([
      'voxtral-mini-transcribe-realtime-3-0',
      'voxtral-mini-transcribe-realtime-3',
    ])
    expect(vendor?.ttsVoiceIds).toBe('open')
    // The defaults are valid for their models.
    expect(
      validateVoiceCatalogChoice('mistral', 'ttsVoice', 'voice_478', 'voxtral-mini-tts-3-0'),
    ).toBeNull()
    expect(validateVoiceCatalogChoice('mistral', 'ttsModel', 'voxtral-mini-tts-3-0')).toBeNull()
    expect(
      validateVoiceCatalogChoice('mistral', 'ttsModel', 'voxtral-mini-tts-latest'),
    ).not.toBeNull()
    expect(
      validateVoiceCatalogChoice('mistral', 'sttModel', 'voxtral-mini-transcribe-realtime-3-0'),
    ).toBeNull()
  })

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

  test('fishaudio models', () => {
    expect(getVendor('fishaudio')?.defaultTtsModel).toBe('drama-3')
    expect(ttsModelAllowlist('fishaudio')).toEqual([
      'drama-3',
      's2.1-pro',
      's2.1-pro-free',
      's2-pro',
    ])
    expect(ttsModelAllowlist('fishaudio')).not.toContain('s1')
    expect(getVendor('fishaudio')?.stt).toBe(true)
    expect(getVendor('fishaudio')?.defaultSttModel).toBe('transcribe-1')
    expect(sttModelAllowlist('fishaudio')).toEqual(['transcribe-1'])
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
    expect(ttsVoiceAllowlist('aws').length).toBe(63)
    expect(ttsVoiceAllowlist('aws')).toEqual(expect.arrayContaining(['Joanna', 'Matthew', 'Amy']))
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
  test('postbuild patch script rewrites every catalog JSON import', () => {
    const src = readFileSync(
      new URL('../scripts/patch-esm-json-imports.mjs', import.meta.url),
      'utf8',
    )
    expect(src).toContain("from '../catalog/vendors.json' with { type: 'json' }")
    expect(src).toContain("from '../catalog/azure-tts-voices.json' with { type: 'json' }")
    expect(src).toContain("from '../catalog/google-tts-voices.json' with { type: 'json' }")
  })
})

describe('documented TTS voice lists', () => {
  test('every customer TTS vendor declares ttsVoiceIds', () => {
    for (const id of customerTtsProviderIds()) {
      expect(['listed', 'open', 'model'], `vendor ${id}`).toContain(getVendor(id)?.ttsVoiceIds)
    }
    expect(getVendor('mock')?.ttsVoiceIds).toBeUndefined()
  })

  test('listed vendors have a non-empty voice list', () => {
    const listed = VOICE_VENDORS.filter((v) => v.ttsVoiceIds === 'listed')
    expect(listed.map((v) => v.id).sort()).toEqual(
      ['aws', 'azure', 'google', 'groq', 'openai'].sort(),
    )
    for (const vendor of listed) {
      expect(ttsVoiceAllowlist(vendor.id).length, `vendor ${vendor.id}`).toBeGreaterThan(0)
    }
  })

  test('defaultTtsVoice is valid for defaultTtsModel', () => {
    for (const vendor of VOICE_VENDORS) {
      if (!vendor.defaultTtsVoice) continue
      expect(
        validateVoiceCatalogChoice(
          vendor.id,
          'ttsVoice',
          vendor.defaultTtsVoice,
          vendor.defaultTtsModel,
        ),
        `vendor ${vendor.id}`,
      ).toBeNull()
    }
  })

  test('tts-1 rejects marin, gpt-4o-mini-tts accepts it', () => {
    expect(validateVoiceCatalogChoice('openai', 'ttsVoice', 'marin', 'tts-1')).toMatchObject({
      kind: 'ttsVoice',
      vendorId: 'openai',
    })
    expect(validateVoiceCatalogChoice('openai', 'ttsVoice', 'marin', 'tts-1-hd')).not.toBeNull()
    expect(validateVoiceCatalogChoice('openai', 'ttsVoice', 'marin', 'gpt-4o-mini-tts')).toBeNull()
    expect(validateVoiceCatalogChoice('openai', 'ttsVoice', 'marin')).toBeNull()
  })

  test('groq arabic model rejects troy and accepts fahad', () => {
    const arabic = 'canopylabs/orpheus-arabic-saudi'
    expect(validateVoiceCatalogChoice('groq', 'ttsVoice', 'troy', arabic)).not.toBeNull()
    expect(validateVoiceCatalogChoice('groq', 'ttsVoice', 'fahad', arabic)).toBeNull()
    expect(
      validateVoiceCatalogChoice('groq', 'ttsVoice', 'troy', 'canopylabs/orpheus-v1-english'),
    ).toBeNull()
    expect(ttsVoiceAllowlist('groq').length).toBe(12)
  })

  test('google snapshot has en-US-Neural2-A and en-US-Chirp3-HD-Charon and no bare Kore', () => {
    expect(googleTtsVoices.length).toBe(2002)
    expect(googleTtsVoices).toContain('en-US-Neural2-A')
    expect(googleTtsVoices).toContain('en-US-Chirp3-HD-Charon')
    expect(googleTtsVoices).not.toContain('Kore')
    expect(googleTtsVoices.every((name) => name.includes('-'))).toBe(true)
    expect(getVendor('google')?.ttsVoiceListFile).toBe('google-tts-voices.json')
    expect(validateVoiceCatalogChoice('google', 'ttsVoice', 'Kore')).not.toBeNull()
    expect(validateVoiceCatalogChoice('google', 'ttsVoice', 'en-US-Neural2-A')).toBeNull()
  })

  test('aws accepts Danielle, rejects standard-only Nicole', () => {
    expect(validateVoiceCatalogChoice('aws', 'ttsVoice', 'Danielle')).toBeNull()
    expect(validateVoiceCatalogChoice('aws', 'ttsVoice', 'Nicole')).not.toBeNull()
  })

  test('deepgram accepts aura-2-thalia-en and flux-hannah-en as ttsModel, rejects aura-nobody-en', () => {
    expect(ttsModelAllowlist('deepgram').length).toBe(138)
    expect(ttsModelAllowlist('deepgram')[0]).toBe('aura-asteria-en')
    expect(getVendor('deepgram')?.ttsVoiceIds).toBe('model')
    expect(validateVoiceCatalogChoice('deepgram', 'ttsModel', 'aura-2-thalia-en')).toBeNull()
    expect(validateVoiceCatalogChoice('deepgram', 'ttsModel', 'flux-hannah-en')).toBeNull()
    expect(validateVoiceCatalogChoice('deepgram', 'ttsModel', 'aura-nobody-en')).not.toBeNull()
  })

  test('open vendors accept any voice id', () => {
    for (const id of ['cartesia', 'elevenlabs', 'fishaudio', 'mistral', 'local-sherpa']) {
      expect(getVendor(id)?.ttsVoiceIds).toBe('open')
      expect(validateVoiceCatalogChoice(id, 'ttsVoice', 'any-customer-clone')).toBeNull()
    }
  })
})

describe('voice catalog transports', () => {
  test('deepgram lists two TTS transports with websocket as default', () => {
    expect([...ttsTransports('deepgram')]).toEqual(['websocket', 'http'])
    expect(defaultTtsTransport('deepgram')).toBe('websocket')
    expect([...sttTransports('deepgram')]).toEqual(['websocket'])
    expect(defaultSttTransport('deepgram')).toBe('websocket')
  })

  // Defaults come from the 2026-10-19 vendor bench (benchmarks/METHODOLOGY.md, "Default transport rule").
  test('elevenlabs defaults to websocket TTS', () => {
    expect([...ttsTransports('elevenlabs')]).toEqual(['http', 'websocket'])
    expect(defaultTtsTransport('elevenlabs')).toBe('websocket')
  })

  test('cartesia defaults to http TTS', () => {
    expect([...ttsTransports('cartesia')]).toEqual(['websocket', 'http'])
    expect(defaultTtsTransport('cartesia')).toBe('http')
  })

  test('mistral: http TTS and websocket STT, one transport each', () => {
    expect([...ttsTransports('mistral')]).toEqual(['http'])
    expect(defaultTtsTransport('mistral')).toBe('http')
    expect([...sttTransports('mistral')]).toEqual(['websocket'])
    expect(defaultSttTransport('mistral')).toBe('websocket')
  })

  test('fish audio keeps http TTS as the default', () => {
    expect(defaultTtsTransport('fishaudio')).toBe('http')
  })

  test('google and unknown vendors have no selectable transport', () => {
    for (const id of ['google', 'not-a-vendor', 'local-sherpa', 'mock']) {
      expect(ttsTransports(id)).toEqual([])
      expect(sttTransports(id)).toEqual([])
      expect(defaultTtsTransport(id)).toBeUndefined()
      expect(defaultSttTransport(id)).toBeUndefined()
    }
  })

  test('default is in the list for every vendor that has one, and absent otherwise', () => {
    for (const vendor of VOICE_VENDORS) {
      for (const [list, def] of [
        [ttsTransports(vendor.id), defaultTtsTransport(vendor.id)],
        [sttTransports(vendor.id), defaultSttTransport(vendor.id)],
      ] as const) {
        if (list.length > 0) {
          expect(def, vendor.id).toBeDefined()
          expect(list, vendor.id).toContain(def)
        } else {
          expect(def, vendor.id).toBeUndefined()
        }
        for (const t of list) expect(['websocket', 'http']).toContain(t)
      }
    }
  })
})
