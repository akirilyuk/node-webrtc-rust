import { describe, expect, it } from 'vitest'

import catalog from '../../shared/sherpa-tts-model-catalog.json'

describe('sherpa TTS catalog', () => {
  it('includes en-lessac-high for e2e / load-staging downloads', () => {
    const lessacHigh = catalog.models.find((entry) => entry.id === 'en-lessac-high')
    const enDefault = catalog.models.find((entry) => entry.id === 'en')
    expect(lessacHigh?.bundle).toBe('vits-piper-en_US-lessac-high')
    expect(enDefault?.bundle).toBe('vits-piper-en_US-amy-low')
    expect(catalog.defaultModelId).toBe('en')
  })

  it('includes Kokoro and Kitten voices with their model kinds', () => {
    const kokoro = catalog.models.find((entry) => entry.id === 'en-kokoro-heart')
    const kitten = catalog.models.find((entry) => entry.id === 'en-kitten-f2')
    expect(kokoro?.kind).toBe('kokoro')
    expect(kokoro?.bundle).toBe('kokoro-multi-lang-v1_0')
    expect(kitten?.kind).toBe('kitten')
    expect(kitten?.bundle).toBe('kitten-nano-en-v0_8-fp32')
  })
})
