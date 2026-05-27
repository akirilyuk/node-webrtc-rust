#!/usr/bin/env bash
# Fast TS quality gates — no native .node required (workspace npm ci only).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "==> npm ci"
npm ci

echo "==> typecheck (sdk + signaling sources)"
npx tsc --noEmit -p scripts/ci/tsconfig.typecheck.json

echo "==> lint"
npm run lint

echo "==> Quality checks OK"
