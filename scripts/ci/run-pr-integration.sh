#!/usr/bin/env bash
# Native + integration tests — requires linux-gnu .node in packages/bindings/.
# Expects quality to have passed. Native .node and TS dist may come from CI caches
# (compile-native / build-ts jobs) or are built here on cache miss.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "==> npm ci"
bash scripts/ci/npm-ci-workspace.sh

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

echo "==> cargo test (core, mixer, conference)"
echo "    (compiles Rust test deps on cold cache — separate from the .node binding above)"
cargo test -p node-webrtc-rust-core
cargo test -p node-webrtc-rust-mixer
cargo test -p node-webrtc-rust-conference

if [[ ! -f packages/sdk/dist/cjs/index.js ]] || [[ ! -f packages/signaling/dist/cjs/index.js ]] || [[ ! -f packages/helpers/dist/cjs/index.js ]]; then
  echo "==> build:ts (dist cache miss)"
  bash scripts/ci/build-ts-workspace.sh
else
  echo "==> build:ts skipped (dist restored from cache)"
fi

echo "==> npm test"
npm test

echo "==> Integration tests OK"
