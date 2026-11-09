import { defineConfig } from 'vitest/config'
import { workerCap } from '../../scripts/vitest/cap'

export default defineConfig(async () => ({
  test: {
    include: ['tests/**/*.test.ts'],
    // Host-wide worker cap on a dev machine; {} in CI. See scripts/vitest/cap.ts.
    ...(await workerCap()),
  },
}))
