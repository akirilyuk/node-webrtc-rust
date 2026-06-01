#!/usr/bin/env bash
# Run helpers vitest when the diff touches helpers or the multi-client Sherpa example.
#
# Usage (from repo root):
#   bash scripts/ci/run-if-helpers-changed.sh              # vs origin/main
#   bash scripts/ci/run-if-helpers-changed.sh --cached   # staged only
#   bash scripts/ci/run-if-helpers-changed.sh origin/main...HEAD
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

RANGE="${1:-origin/main...HEAD}"
if [[ "$RANGE" == "--cached" ]]; then
  FILES="$(git diff --cached --name-only)"
else
  FILES="$(git diff --name-only "$RANGE" 2>/dev/null || true)"
fi

if echo "$FILES" | grep -qE '^packages/helpers/|^examples/voice-agent-local-sherpa-multi-client/'; then
  echo "==> helpers-related paths changed — running unit tests"
  bash "$ROOT/scripts/ci/run-helpers-unit-tests.sh"
else
  echo "==> no helpers / multi-client example changes in range ($RANGE) — skip"
fi
