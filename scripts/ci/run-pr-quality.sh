#!/usr/bin/env bash
# Fast TS quality gates — no native .node required (workspace npm ci only).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "==> npm ci"
npm ci

echo "==> typecheck (sdk + signaling sources)"
npx tsc --noEmit -p scripts/ci/tsconfig.typecheck.json

echo "==> build-ts-workspace (release publish path; catches stale npm bindings types)"
bash scripts/ci/build-ts-workspace.sh

echo "==> lint"
npm run lint

echo "==> helpers unit tests (vitest, no native .node)"
bash scripts/ci/run-helpers-unit-tests.sh

echo "==> Quality checks OK"
