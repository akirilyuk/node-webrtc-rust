import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    environment: 'node',
    testTimeout: 60_000,
    hookTimeout: 60_000,
    // WebRTC peer tests flake when files run in parallel (shared ICE ports) — same as
    // `cargo test … peer_connection_test -- --test-threads=1` in run-pr-integration.sh.
    fileParallelism: false,
  },
})
