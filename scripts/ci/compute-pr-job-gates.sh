#!/usr/bin/env bash
# Compute PR job gates (compile / build-ts / test / quality) from changed paths.
#
# Usage (CI):
#   compute-pr-job-gates.sh "$PR_BASE_SHA" "$PR_HEAD_SHA" "$FILTER_NATIVE" "$FILTER_WORKFLOWS_NATIVE"
#
# Uses merge-base..head for the full PR diff AND files touched by the latest head commit
# so a push that only edits crates/speech still forces native compile + integration tests.
set -euo pipefail

PR_BASE_SHA="${1:?PR base SHA (main tip)}"
PR_HEAD_SHA="${2:?PR head SHA (branch tip — use pull_request.head.sha, not github.sha)}"
FILTER_NATIVE="${3:-false}"
FILTER_WORKFLOWS_NATIVE="${4:-false}"

run_compile=false
run_build_ts=false
run_test=false
run_quality=false

classify_path() {
  local path="$1"
  [[ -z "$path" ]] && return 0
  [[ "$path" == *.md || "$path" == docs/* ]] && return 0

  if [[ "$path" == .github/workflows/* || "$path" == .cursor/* ]]; then
    run_quality=true
    return 0
  fi
  if [[ "$path" == .github/actions/ci-build-native-* || "$path" == .github/actions/native-binding-cache/* || "$path" == docker/ci/* || "$path" == scripts/ci/native-binding-cache-key.sh || "$path" == scripts/ci/verify-native-binding-surface.mjs ]]; then
    run_compile=true
    run_quality=true
    return 0
  fi
  if [[ "$path" == .github/actions/ci-cache-ts-dist/* || "$path" == scripts/ci/build-ts-workspace.sh || "$path" == scripts/ci/verify-release-publish-ts.sh || "$path" == scripts/ci/ts-dist-cache-key.sh ]]; then
    run_build_ts=true
    run_quality=true
    return 0
  fi
  if [[ "$path" == .github/actions/ci-run-integration-tests/* || "$path" == .github/workflows/reusable-test.yml || "$path" == scripts/ci/run-pr-integration.sh || "$path" == scripts/ci/run-pr-tests-full.sh || "$path" == scripts/ci/run-sherpa-example-ci.sh || "$path" == scripts/ci/run-sherpa-roundtrip-e2e.sh || "$path" == scripts/ci/run-pr-test-job-docker.sh || "$path" == scripts/ci/compute-pr-job-gates.sh ]]; then
    run_quality=true
    return 0
  fi
  if [[ "$path" == Cargo.toml || "$path" == Cargo.lock || "$path" =~ ^crates/ || "$path" =~ ^packages/bindings/ ]]; then
    run_compile=true
    run_test=true
    run_quality=true
    return 0
  fi
  if [[ "$path" =~ ^packages/sdk/ || "$path" =~ ^packages/signaling/ || "$path" =~ ^packages/helpers/ || "$path" == package.json || "$path" == package-lock.json || "$path" == eslint.config.js || "$path" == .prettierrc* || "$path" == *tsconfig*.json ]]; then
    run_build_ts=true
    run_test=true
    run_quality=true
    return 0
  fi
  if [[ "$path" =~ ^examples/ ]]; then
    run_build_ts=true
    run_test=true
    run_quality=true
    return 0
  fi
  if [[ "$path" == scripts/ci/* ]]; then
    run_quality=true
    return 0
  fi

  run_quality=true
  run_test=true
}

merge_base="$(git merge-base "$PR_BASE_SHA" "$PR_HEAD_SHA" 2>/dev/null || echo "$PR_BASE_SHA")"

while IFS= read -r path; do
  classify_path "$path"
done < <(git diff --name-only "$merge_base" "$PR_HEAD_SHA")

# Latest commit on the PR branch (covers speech-only pushes after merge-base is stale).
if git rev-parse --verify "${PR_HEAD_SHA}^{commit}" >/dev/null 2>&1; then
  while IFS= read -r path; do
    classify_path "$path"
  done < <(git diff-tree --no-commit-id --name-only -r "$PR_HEAD_SHA" 2>/dev/null || true)
fi

# dorny/paths-filter native / workflows_native (same PR scope, independent implementation).
if [[ "$FILTER_NATIVE" == "true" || "$FILTER_WORKFLOWS_NATIVE" == "true" ]]; then
  run_compile=true
  run_test=true
  run_quality=true
fi

emit_bool() {
  local name="$1"
  local value="$2"
  if [[ "$value" == true ]]; then
    echo "${name}=true"
  else
    echo "${name}=false"
  fi
}

emit_bool run_compile "$run_compile"
emit_bool run_build_ts "$run_build_ts"
emit_bool run_test "$run_test"
emit_bool run_quality "$run_quality"

if [[ "$run_test" == true ]]; then
  echo "skip_test=false"
  echo "needs_native=true"
  echo "needs_ts=true"
else
  echo "skip_test=true"
  echo "needs_native=false"
  echo "needs_ts=false"
fi

if [[ "$run_quality" == true ]]; then
  echo "skip_quality=false"
else
  echo "skip_quality=true"
fi

echo "::notice::PR gates merge_base=${merge_base:0:12} head=${PR_HEAD_SHA:0:12} — compile=${run_compile} build_ts=${run_build_ts} test=${run_test} quality=${run_quality}"
