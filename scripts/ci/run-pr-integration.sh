#!/usr/bin/env bash
# Native + integration tests — requires linux-gnu .node in packages/bindings/.
# Assumes typecheck/lint already passed in the quality job (PR) or run-pr-tests-full.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "==> npm ci"
npm ci

echo "==> cargo test (core, mixer, conference)"
cargo test -p node-webrtc-rust-core
cargo test -p node-webrtc-rust-mixer
cargo test -p node-webrtc-rust-conference

echo "==> build:ts"
npm run build:ts

echo "==> npm test"
npm test

echo "==> Integration tests OK"
