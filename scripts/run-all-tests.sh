#!/usr/bin/env bash
# Run every test in the repo: all Cargo workspace crates + npm workspaces with a test script.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

shopt -s nullglob
nodes=(packages/bindings/*.node)
if [[ ${#nodes[@]} -eq 0 ]]; then
  echo "No native .node in packages/bindings — run: npm run build:native" >&2
  exit 1
fi

echo "==> cargo test (all workspace crates)"
cargo test --workspace

echo "==> npm test (all workspaces with a test script)"
npm run test --workspaces --if-present

echo "==> All tests OK"
