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
})
