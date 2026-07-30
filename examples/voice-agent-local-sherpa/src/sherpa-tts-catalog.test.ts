import { describe, expect, it } from 'vitest'

import { getSherpaTtsModelEntry } from '../scripts/sherpa-tts-model-catalog.mjs'

describe('sherpa TTS catalog', () => {
  it('includes en-lessac-high for e2e / load-staging downloads', () => {
    const entry = getSherpaTtsModelEntry('en-lessac-high')
    expect(entry?.bundle).toBe('vits-piper-en_US-lessac-high')
    expect(getSherpaTtsModelEntry('en')?.bundle).toBe('vits-piper-en_US-amy-low')
  })
})
