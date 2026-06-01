#!/usr/bin/env bash
# Fast local gates before push — mirrors the failures that break CI quality job.
#
# Usage (from repo root):
#   npm run ci:pre-push
#   bash scripts/ci/run-pre-push-gates.sh              # vs origin/main
#   bash scripts/ci/run-pre-push-gates.sh --cached     # staged only
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

RANGE="${1:-origin/main...HEAD}"
if [[ "$RANGE" == "--cached" ]]; then
  FILES="$(git diff --cached --name-only)"
else
  FILES="$(git diff --name-only "$RANGE" 2>/dev/null || true)"
fi

if [[ -z "$FILES" ]]; then
  echo "==> no changed files in range ($RANGE) — skip"
  exit 0
fi

NEED_LINT=false
NEED_HELPERS=false
NEED_TS_BUILD=false

if echo "$FILES" | grep -qE \
  '^(packages/|examples/.*\.ts$|eslint\.config|tsconfig\.eslint|scripts/ci/|\.github/workflows/)'; then
  NEED_LINT=true
fi

if echo "$FILES" | grep -qE '^packages/helpers/|^examples/voice-agent-local-sherpa-multi-client/'; then
  NEED_HELPERS=true
fi

# Rebuild workspace when sdk/signaling/helpers sources change — catches failures that
# only show up on CI (fresh npm ci, no dist) if local stale dist would skip the build.
if echo "$FILES" | grep -qE '^packages/(helpers|sdk|signaling)/'; then
  NEED_TS_BUILD=true
  NEED_HELPERS=true
fi

if ! $NEED_LINT && ! $NEED_HELPERS && ! $NEED_TS_BUILD; then
  echo "==> no eslint/helpers-scoped changes in range ($RANGE) — skip"
  exit 0
fi

if $NEED_LINT; then
  echo "==> eslint (same as run-pr-quality.sh lint step)"
  npm run lint
fi

if $NEED_TS_BUILD; then
  echo "==> build TypeScript workspace (changed sdk/signaling/helpers — match CI)"
  bash "$ROOT/scripts/ci/build-ts-workspace.sh"
fi

if $NEED_HELPERS; then
  echo "==> helpers vitest"
  bash "$ROOT/scripts/ci/run-helpers-unit-tests.sh"
fi

echo "==> Pre-push gates OK"
