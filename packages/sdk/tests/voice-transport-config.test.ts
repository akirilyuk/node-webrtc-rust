import { describe, expect, test } from 'vitest'

import { toJsSttConfig, toJsTtsConfig } from '../src/voice/VoiceAgent'

describe('SDK transport → NAPI config', () => {
  test('tts transport reaches the native config unchanged', () => {
    expect(toJsTtsConfig({ provider: 'deepgram', transport: 'http' }).transport).toBe('http')
    expect(toJsTtsConfig({ provider: 'elevenlabs', transport: 'websocket' }).transport).toBe(
      'websocket',
    )
    expect(toJsTtsConfig({ provider: 'cartesia', transport: 'auto' }).transport).toBe('auto')
  })

  test('stt transport reaches the native config unchanged', () => {
    expect(toJsSttConfig({ provider: 'deepgram', transport: 'websocket' }).transport).toBe(
      'websocket',
    )
  })

  test('omitted transport stays undefined (catalog default)', () => {
    expect(toJsTtsConfig({ provider: 'deepgram' }).transport).toBeUndefined()
    expect(toJsSttConfig({ provider: 'deepgram' }).transport).toBeUndefined()
  })
})
