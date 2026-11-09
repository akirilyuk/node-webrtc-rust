import { defineConfig } from 'vitest/config'
import { workerCap } from '../../scripts/vitest/cap'

export default defineConfig(async () => ({
  test: {
    environment: 'node',
    testTimeout: 60_000,
    hookTimeout: 60_000,
    // Native bindings are not safe to share across vitest worker threads.
    pool: 'forks',
    fileParallelism: false,
    // Files run serially, so one slot is enough. Host-wide worker cap on a dev machine; {} in CI. See scripts/vitest/cap.ts.
    ...(await workerCap({ want: 1 })),
  },
}))
