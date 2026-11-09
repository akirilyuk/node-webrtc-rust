import { defineConfig } from 'vitest/config'
import { workerCap } from '../../scripts/vitest/cap'

// Only applies the worker cap; everything else stays at vitest defaults.
export default defineConfig(async () => ({
  test: { ...(await workerCap()) },
}))
