import { describe, expect, test } from 'vitest'

import { toJsConfig } from '../src/voice/VoiceAgent'

describe('languageId.reportOutOfSet', () => {
  test('passes reportOutOfSet through', () => {
    const js = toJsConfig({
      languageId: { modelPath: 'm', allowlist: ['en'], reportOutOfSet: true },
    })
    expect(js?.languageId?.reportOutOfSet).toBe(true)
  })

  test('omitted reportOutOfSet stays undefined', () => {
    const js = toJsConfig({ languageId: { modelPath: 'm', allowlist: ['en'] } })
    expect(js?.languageId?.reportOutOfSet).toBeUndefined()
  })
})
