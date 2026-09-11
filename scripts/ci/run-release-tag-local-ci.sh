#!/usr/bin/env bash
# Required local CI before `git tag release/X.Y.Z`.
#
# A green release-prep PR is NOT enough: PR Typecheck can skip eslint .
# (detect-changes). Release always runs run-pr-quality.sh.
# Lessons: 0.8.1 (consistent-type-imports), 0.9.6 (unused CLIP_RMS_PROBE_MS).
#
# Does not require ci:verify:release-ts. That script may fail with ETARGET for
# @node-webrtc-rust/bindings@X.Y.Z before the first npm publish — expected.
# Do not skip this script because of that failure.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
if [[ "$BRANCH" != "main" ]]; then
  echo "run-release-tag-local-ci: must be on main (got ${BRANCH})" >&2
  exit 1
fi

echo "==> fetch origin/main"
git fetch origin main
HEAD_SHA="$(git rev-parse HEAD)"
ORIGIN_MAIN="$(git rev-parse origin/main)"
if [[ "$HEAD_SHA" != "$ORIGIN_MAIN" ]]; then
  echo "run-release-tag-local-ci: HEAD is not origin/main. Pull/rebase first." >&2
  echo "  HEAD=${HEAD_SHA}" >&2
  echo "  origin/main=${ORIGIN_MAIN}" >&2
  exit 1
fi

mkdir -p .test-logs
STAMP="$(date +%Y%m%d-%H%M%S)"

run_step() {
  local label="$1"
  local log="$2"
  shift
  echo "==> ${label} -> ${log}"
  if "$@" >"$log" 2>&1; then
    echo "exit=0" >>"$log"
    return 0
  else
    local ec=$?
    echo "exit=${ec}" >>"$log"
    echo "FAIL ${label} (exit ${ec}); search ${log}" >&2
    return "$ec"
  fi
}

QLOG=".test-logs/${STAMP}-pr-quality.log"
NLOG=".test-logs/${STAMP}-build-native.log"
TLOG=".test-logs/${STAMP}-pr-tests-full.log"

run_step "run-pr-quality.sh" "$QLOG" bash scripts/ci/run-pr-quality.sh
run_step "build:native" "$NLOG" npm run build:native
run_step "run-pr-tests-full.sh" "$TLOG" bash scripts/ci/run-pr-tests-full.sh

bash scripts/ci/write-release-tag-gate.sh 0 0 0
echo "OK: local release-tag CI passed for ${HEAD_SHA}"
echo "You may now: git tag release/X.Y.Z && git push origin refs/tags/release/X.Y.Z"
