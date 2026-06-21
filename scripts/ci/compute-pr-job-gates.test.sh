#!/usr/bin/env bash
# Shell tests for compute-pr-job-gates.sh (no GitHub Actions required).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

GATES="$ROOT/scripts/ci/compute-pr-job-gates.sh"
chmod +x "$GATES"

base="$(git merge-base origin/main HEAD 2>/dev/null || git rev-parse origin/main 2>/dev/null || git rev-parse HEAD~1)"
head="$(git rev-parse HEAD)"

out="$("$GATES" "$base" "$head" false false)"

assert_contains() {
  local haystack="$1"
  local needle="$2"
  if [[ "$haystack" != *"$needle"* ]]; then
    echo "expected output to contain: $needle" >&2
    echo "$haystack" >&2
    exit 1
  fi
}

# Touching speech in the latest commit must force compile + test when present in diff-tree.
if git diff-tree --no-commit-id --name-only -r "$head" | rg -q '^crates/speech/'; then
  assert_contains "$out" "run_compile=true"
  assert_contains "$out" "run_test=true"
  assert_contains "$out" "skip_test=false"
  echo "ok: latest commit touches crates/speech → compile + test"
else
  echo "skip: latest commit does not touch crates/speech (manual check only)"
fi

# Simulated speech-only latest commit via env override is not possible without a fixture;
# verify filter fallback forces native jobs.
out_filter="$("$GATES" "$base" "$base" true false)"
assert_contains "$out_filter" "run_compile=true"
assert_contains "$out_filter" "skip_test=false"
echo "ok: paths-filter native fallback forces compile + test"

echo "compute-pr-job-gates.test.sh: all checks passed"
