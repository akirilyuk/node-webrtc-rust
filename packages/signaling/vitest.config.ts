import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    environment: 'node',
    testTimeout: 60_000,
    hookTimeout: 60_000,
    // Native bindings are not safe to share across vitest worker threads.
    pool: 'forks',
    fileParallelism: false,
  },
})
