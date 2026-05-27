#!/usr/bin/env bash
# Native + integration tests — requires linux-gnu .node in packages/bindings/.
# Expects quality to have passed. Native .node and TS dist may come from CI caches
# (compile-native / build-ts jobs) or are built here on cache miss.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "==> npm ci"
npm ci

echo "==> ensure native binding"
shopt -s nullglob
nodes=(packages/bindings/*.node)
if [[ ${#nodes[@]} -eq 0 ]]; then
  echo "    cache miss — compiling debug linux-gnu binding"
  ( cd packages/bindings && npx napi build --target x86_64-unknown-linux-gnu )
  shopt -s nullglob
  nodes=(packages/bindings/*.node)
  if [[ ${#nodes[@]} -eq 0 ]]; then
    echo "No .node artifact after fallback compile." >&2
    exit 1
  fi
fi

echo "==> cargo test (core, mixer, conference)"
cargo test -p node-webrtc-rust-core
cargo test -p node-webrtc-rust-mixer
cargo test -p node-webrtc-rust-conference

if [[ ! -f packages/sdk/dist/cjs/index.js ]] || [[ ! -f packages/signaling/dist/cjs/index.js ]]; then
  echo "==> build:ts (dist cache miss)"
  npm run build:ts
else
  echo "==> build:ts skipped (dist restored from cache)"
fi

echo "==> npm test"
npm test

echo "==> Integration tests OK"
