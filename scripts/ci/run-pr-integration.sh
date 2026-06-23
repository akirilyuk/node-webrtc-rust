#!/usr/bin/env bash
# Native + integration tests — requires linux-gnu .node in packages/bindings/.
# Expects quality to have passed. Native .node and TS dist may come from CI caches
# (compile-native / build-ts jobs) or are built here on cache miss.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

CI_STEP="$ROOT/scripts/ci/ci-step.sh"
DEFAULT_CARGO_LIB_TIMEOUT_SEC="${CI_CARGO_LIB_TIMEOUT_SEC:-180}"
DEFAULT_CARGO_PEER_TEST_TIMEOUT_SEC="${CI_CARGO_PEER_TEST_TIMEOUT_SEC:-420}"
DEFAULT_NPM_TEST_TIMEOUT_SEC="${CI_NPM_TEST_TIMEOUT_SEC:-600}"

echo "==> npm ci"
bash scripts/ci/npm-ci-workspace.sh

echo "==> integration git $(git rev-parse --short HEAD)"

echo "==> ensure native binding"
shopt -s nullglob
nodes=(packages/bindings/*.node)
if [[ ${#nodes[@]} -eq 0 ]]; then
  if [[ "${CI:-}" == "true" ]]; then
    echo "No .node in CI workspace — compile-native artifact or cache should have restored it." >&2
    exit 1
  fi
  echo "    no .node in workspace — compiling debug linux-gnu binding (local fallback)"
  ( cd packages/bindings && npx napi build --target x86_64-unknown-linux-gnu && npm run copy:local-node )
  shopt -s nullglob
  nodes=(packages/bindings/*.node)
  if [[ ${#nodes[@]} -eq 0 ]]; then
    echo "No .node artifact after fallback compile." >&2
    exit 1
  fi
else
  echo "    using $(basename "${nodes[0]}") (artifact or cache)"
fi

echo "==> sync workspace bindings for npm test (registry copy under packages/sdk)"
bash "$ROOT/scripts/ci/sync-workspace-bindings.sh"

echo "==> cargo test (core, mixer, conference, speech)"
echo "    (compiles Rust test deps on cold cache — separate from the .node binding above)"
# WebRTC peer integration tests flake when run in parallel (shared ICE/ports).
bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo core --lib" -- \
  cargo test -p node-webrtc-rust-core --lib
bash "$CI_STEP" --timeout "$DEFAULT_CARGO_PEER_TEST_TIMEOUT_SEC" "cargo peer_connection_test" -- \
  cargo test -p node-webrtc-rust-core --test peer_connection_test -- --test-threads=1
bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo mixer" -- \
  cargo test -p node-webrtc-rust-mixer
bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo conference" -- \
  cargo test -p node-webrtc-rust-conference
bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo speech" -- \
  cargo test -p node-webrtc-rust-speech

bash "$ROOT/scripts/ci/ensure-ts-dist.sh"

echo "==> npm test"
bash "$CI_STEP" --timeout "$DEFAULT_NPM_TEST_TIMEOUT_SEC" "npm test" -- npm test

echo "==> Sherpa roundtrip E2E (all start:roundtrip-*)"
bash scripts/ci/run-sherpa-example-ci.sh e2e

echo "==> Integration tests OK"
