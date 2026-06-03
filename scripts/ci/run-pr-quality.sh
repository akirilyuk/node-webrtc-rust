#!/usr/bin/env bash
# Fast TS quality gates — no native .node required (workspace npm ci only).
#
# Does NOT run build-ts-workspace.sh — that runs once in CI "Build TypeScript"
# (ci-cache-ts-dist) and again in run-pr-integration.sh only on cache miss.
# For release-publish compile parity locally: bash scripts/ci/verify-release-publish-ts.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "==> validate package-lock optional bindings"
bash scripts/ci/validate-package-lock-optional-bindings.sh

echo "==> npm ci"
npm ci

echo "==> rollup native binary (Linux CI — npm optional-deps bug)"
bash scripts/fix-rollup-native.sh

echo "==> typecheck (sdk + signaling + helpers sources)"
npx tsc --noEmit -p scripts/ci/tsconfig.typecheck.json

echo "==> lint"
npm run lint

echo "==> helpers unit tests (vitest, no native .node)"
bash scripts/ci/run-helpers-unit-tests.sh

echo "==> Sherpa example typecheck"
bash scripts/ci/run-sherpa-example-ci.sh typecheck

echo "==> Sherpa roundtrip Vitest evaluators"
bash scripts/ci/run-sherpa-example-ci.sh vitest

echo "==> Quality checks OK"
