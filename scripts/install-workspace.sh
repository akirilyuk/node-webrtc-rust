#!/usr/bin/env bash
# Install all npm workspace dependencies from the repo root.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "==> npm install (root + workspaces)"
npm install

bash scripts/fix-rollup-native.sh

echo "==> workspace install complete"
