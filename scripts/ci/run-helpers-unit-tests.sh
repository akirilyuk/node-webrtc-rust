#!/usr/bin/env bash
# Vitest for @node-webrtc-rust/helpers and its multi-client example (no native .node).
# Called from run-pr-quality.sh (PR + main quality job) and via npm run test:helpers.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if [[ -d node_modules/rollup ]]; then
  bash "$ROOT/scripts/fix-rollup-native.sh"
fi

if [[ ! -f packages/helpers/dist/cjs/index.js ]]; then
  echo "==> build helpers (dist missing)"
  npm run build --workspace=@node-webrtc-rust/helpers
fi

echo "==> vitest @node-webrtc-rust/helpers"
npm run test --workspace=@node-webrtc-rust/helpers

echo "==> vitest example-voice-agent-local-sherpa-multi-client"
npm run test --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client

echo "==> Helpers unit tests OK"
