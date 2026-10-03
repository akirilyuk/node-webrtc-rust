import { describe, expect, test } from 'vitest'

import { preloadLanguageId } from '../src/voice'

const lidModelPath = process.env.SHERPA_LID_MODEL_PATH

describe('preloadLanguageId', () => {
  test('rejects when modelPath is missing', async () => {
    await expect(preloadLanguageId({})).rejects.toThrow(/modelPath/)
  })

  test('rejects when modelPath is not a directory', async () => {
    await expect(
      preloadLanguageId({ modelPath: '/nonexistent/nwr-lid-model-dir' }),
    ).rejects.toThrow(/not a directory/)
  })

  // Needs the Whisper tiny bundle (CI: scripts/ci/run-sherpa-example-ci.sh exports the path).
  test.skipIf(!lidModelPath)('loads the model and is idempotent', async () => {
    const config = { modelPath: lidModelPath }
    await preloadLanguageId(config)
    const started = Date.now()
    await Promise.all([preloadLanguageId(config), preloadLanguageId(config)])
    // Already resident: a pool lookup, not another ~100 ms+ load.
    expect(Date.now() - started).toBeLessThan(50)
  })
})
