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
NEED_SHERPA_TYPECHECK=false
NEED_SHERPA_VITEST=false
NEED_SHERPA_E2E=false

if echo "$FILES" | grep -qE \
  '^(packages/|examples/.*\.ts$|eslint\.config|tsconfig\.eslint|scripts/ci/|\.github/workflows/)'; then
  NEED_LINT=true
fi

if echo "$FILES" | grep -qE '^packages/helpers/|^examples/voice-agent-local-sherpa-multi-client/'; then
  NEED_HELPERS=true
fi

if echo "$FILES" | grep -qE '^examples/voice-agent-local-sherpa/|^examples/voice-agent/|^examples/shared/'; then
  NEED_SHERPA_TYPECHECK=true
  NEED_SHERPA_VITEST=true
  NEED_SHERPA_E2E=true
  NEED_LINT=true
fi

if echo "$FILES" | grep -qE '^crates/speech/'; then
  NEED_SHERPA_VITEST=true
  NEED_SHERPA_E2E=true
fi

# Rebuild workspace when sdk/signaling/helpers sources change — catches failures that
# only show up on CI (fresh npm ci, no dist) if local stale dist would skip the build.
if echo "$FILES" | grep -qE '^packages/(helpers|sdk|signaling)/'; then
  NEED_TS_BUILD=true
  NEED_HELPERS=true
  NEED_SHERPA_TYPECHECK=true
  NEED_SHERPA_VITEST=true
fi

if echo "$FILES" | grep -qE '^package-lock\.json$'; then
  echo "==> validate package-lock optional bindings"
  bash "$ROOT/scripts/ci/validate-package-lock-optional-bindings.sh"
fi

if ! $NEED_LINT && ! $NEED_HELPERS && ! $NEED_TS_BUILD && ! $NEED_SHERPA_TYPECHECK && ! $NEED_SHERPA_VITEST && ! $NEED_SHERPA_E2E; then
  if echo "$FILES" | grep -qE '^package-lock\.json$'; then
    echo "==> Pre-push gates OK (package-lock validate only)"
    exit 0
  fi
  echo "==> no eslint/helpers/sherpa-scoped changes in range ($RANGE) — skip"
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

if $NEED_SHERPA_TYPECHECK; then
  echo "==> Sherpa example typecheck (CI quality parity)"
  bash "$ROOT/scripts/ci/run-sherpa-example-ci.sh" typecheck
fi

if $NEED_SHERPA_VITEST; then
  echo "==> Sherpa roundtrip Vitest evaluators (CI quality parity)"
  bash "$ROOT/scripts/ci/run-sherpa-example-ci.sh" vitest
fi

if $NEED_SHERPA_E2E; then
  echo "==> Sherpa roundtrip E2E (all start:roundtrip-* — requires .node + models)"
  bash "$ROOT/scripts/ci/run-sherpa-example-ci.sh" e2e
fi

echo "==> Pre-push gates OK"
