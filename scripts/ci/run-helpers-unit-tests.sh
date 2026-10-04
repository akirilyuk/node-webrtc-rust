#!/usr/bin/env bash
# Vitest for @node-webrtc-rust/helpers unit tests (excludes *.integration.test.ts)
# and its multi-client example. Quality job has no native .node; leftover bindings
# on a self-hosted runner must not pull SessionPod mix-smoke into this script.
# Called from run-pr-quality.sh (PR + main quality job) and via npm run test:helpers.
# Native helpers integration: npm run test:integration --workspace=@node-webrtc-rust/helpers
# (run-pr-integration.sh after npm test).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if [[ -d node_modules/rollup ]]; then
  bash "$ROOT/scripts/fix-rollup-native.sh"
fi

# Helpers imports workspace sdk + signaling packages (NodeNext resolution needs their dist/).
# ensure-ts-dist.sh rebuilds when dist is missing OR its source stamp does not match. A
# "missing files only" check let a stale local dist (built before a pull) fail these tests
# against old SDK code while a clean CI tree passed.
bash "$ROOT/scripts/ci/ensure-ts-dist.sh"

echo "==> vitest @node-webrtc-rust/helpers"
npm run test --workspace=@node-webrtc-rust/helpers

echo "==> vitest example-voice-agent-local-sherpa-multi-client"
npm run test --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client

echo "==> Helpers unit tests OK"
