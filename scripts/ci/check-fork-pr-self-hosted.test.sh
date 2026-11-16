#!/usr/bin/env bash
# Tests for check-fork-pr-self-hosted.sh: fixtures for ungated and gated self-hosted PR jobs.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CHECK="$ROOT/scripts/ci/check-fork-pr-self-hosted.sh"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/check-fork-pr-XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

# fixture <name> <on-block> <job-yaml>: writes $TMP/<name>/ci.yml
fixture() {
  mkdir -p "$TMP/$1"
  printf 'name: x\non:\n%s\njobs:\n%s\n' "$2" "$3" >"$TMP/$1/ci.yml"
}
PR='  pull_request:
    branches: [main]'

echo "==> real repo workflows pass"
bash "$CHECK" >/dev/null || fail "guard fails on the real tree"

echo "==> ungated self-hosted PR job fails and is named"
fixture ungated "$PR" "  build:
    runs-on: \${{ vars.NWR_RUNNER_SMALL || 'nwr-small' }}
    steps:
      - run: echo hi"
if out="$(bash "$CHECK" "$TMP/ungated" 2>&1)"; then fail "ungated job passed"; fi
grep -q "ci.yml: job 'build'" <<<"$out" || fail "violation does not name file and job: $out"

echo "==> literal self-hosted label fails"
fixture literal "$PR" "  build:
    runs-on: self-hosted
    steps:
      - run: echo hi"
bash "$CHECK" "$TMP/literal" >/dev/null 2>&1 && fail "self-hosted literal passed"

echo "==> if with a different condition fails"
fixture otherif "$PR" "  build:
    if: github.event_name == 'pull_request'
    runs-on: nwr-small
    steps:
      - run: echo hi"
bash "$CHECK" "$TMP/otherif" >/dev/null 2>&1 && fail "unrelated if passed"

echo "==> same-repo if passes (single line and block)"
fixture gated "$PR" "  build:
    if: github.event.pull_request.head.repo.full_name == github.repository
    runs-on: nwr-small
    steps:
      - run: echo hi
  other:
    if: |
      always() &&
      github.event.pull_request.head.repo.full_name == github.repository
    runs-on: self-hosted
    steps:
      - run: echo hi"
bash "$CHECK" "$TMP/gated" >/dev/null || fail "gated jobs failed"

echo "==> GitHub-hosted literal passes without an if"
fixture hosted "$PR" "  build:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi"
bash "$CHECK" "$TMP/hosted" >/dev/null || fail "hosted job failed"

echo "==> runs-on that sends fork PRs to a hosted label passes"
fixture forkhosted "$PR" "  test:
    runs-on: \${{ github.event.pull_request.head.repo.full_name != github.repository && 'ubuntu-latest' || 'nwr-small' }}
    steps:
      - run: echo hi"
bash "$CHECK" "$TMP/forkhosted" >/dev/null || fail "fork-to-hosted runs-on failed"

echo "==> non-PR workflow is not checked"
fixture push "  push:
    tags: ['release/*']" "  build:
    runs-on: nwr-small
    steps:
      - run: echo hi"
bash "$CHECK" "$TMP/push" >/dev/null || fail "push-only workflow was checked"

echo "==> reusable workflow called from a PR workflow is checked"
mkdir -p "$TMP/reusable"
printf 'name: caller\non:\n  pull_request:\njobs:\n  t:\n    uses: ./.github/workflows/callee.yml\n' >"$TMP/reusable/caller.yml"
printf 'name: callee\non:\n  workflow_call:\njobs:\n  r:\n    runs-on: eu-ci\n    steps:\n      - run: echo hi\n' >"$TMP/reusable/callee.yml"
out="$(bash "$CHECK" "$TMP/reusable" 2>&1)" && fail "ungated reusable job passed"
grep -q "callee.yml: job 'r'" <<<"$out" || fail "reusable violation not named: $out"

echo "ok: check-fork-pr-self-hosted"
