#!/usr/bin/env bash
# Vitest for @node-webrtc-rust/helpers and its multi-client example (no native .node).
# Called from run-pr-quality.sh (PR + main quality job) and via npm run test:helpers.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if [[ -d node_modules/rollup ]]; then
  bash "$ROOT/scripts/fix-rollup-native.sh"
fi

# Helpers imports workspace sdk + signaling packages (NodeNext resolution needs their dist/).
# Building helpers alone fails on a fresh checkout — same as CI Typecheck & lint job.
if [[ ! -f packages/helpers/dist/cjs/index.js ]] \
  || [[ ! -f packages/sdk/dist/cjs/index.js ]] \
  || [[ ! -f packages/signaling/dist/cjs/index.js ]]; then
  echo "==> build TypeScript workspace (sdk → signaling → helpers; dist missing)"
  bash "$ROOT/scripts/ci/build-ts-workspace.sh"
fi

echo "==> vitest @node-webrtc-rust/helpers"
npm run test --workspace=@node-webrtc-rust/helpers

echo "==> vitest example-voice-agent-local-sherpa-multi-client"
npm run test --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client

echo "==> Helpers unit tests OK"
