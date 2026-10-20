#!/usr/bin/env bash
# Compute PR job gates (compile / build-ts / test / quality) from changed paths.
#
# Usage (CI):
#   compute-pr-job-gates.sh "$PR_BASE_SHA" "$PR_HEAD_SHA"
#
# Rules:
#   - Docs only (*.md, docs/**, .cursor/**, LICENSE*): quality only.
#   - Anything else: full pipeline (compile + build-ts + test + quality). Tests always need a
#     freshly compiled .node, so run_test implies run_compile.
#   - Version-only diff (only package.json / package-lock.json, and only our own
#     @node-webrtc-rust/* versions change; see version-only-diff.py): quality + build-ts only.
#     main still runs the full suite after merge.
#   - When tests run, test_suites (comma list of rust,node,sherpa) is the union over the changed
#     non-docs paths of scripts/ci/pr-test-suites.map (first matching rule wins, unmapped path =
#     all three). Empty when no tests run. The PR workflow passes it as CI_TEST_SUITES; main,
#     release and local runs leave it unset and run everything.
#
# Uses merge-base..head for the full PR diff AND files touched by the latest head commit
# so a push that only edits crates/speech still forces native compile + integration tests.
set -euo pipefail

PR_BASE_SHA="${1:?PR base SHA (main tip)}"
PR_HEAD_SHA="${2:?PR head SHA (branch tip — use pull_request.head.sha, not github.sha)}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Fail closed: outside a repo git diff falls back to no-index mode and prints nothing useful, which
# would classify every PR as docs-only (e.g. dubious ownership in a container job).
if ! git rev-parse --git-dir >/dev/null 2>&1; then
  echo "::error::compute-pr-job-gates: $(pwd) is not a git repository (safe.directory?)" >&2
  exit 1
fi

run_compile=false
run_build_ts=false
run_test=false
run_quality=false
code_paths=()

classify_path() {
  local path="$1"
  [[ -z "$path" ]] && return 0
  # Docs only: Prettier (quality) covers Markdown; nothing to compile or test.
  if [[ "$path" == *.md || "$path" == docs/* || "$path" == .cursor/* || "$path" == LICENSE* ]]; then
    run_quality=true
    return 0
  fi
  # Everything else runs the full PR pipeline. Tests always need a freshly compiled .node:
  # a PR-scoped debug binding cache from another PR is not visible, so run_test implies run_compile.
  run_compile=true
  run_build_ts=true
  run_test=true
  run_quality=true
  code_paths+=("$path")
}

merge_base="$(git merge-base "$PR_BASE_SHA" "$PR_HEAD_SHA" 2>/dev/null || echo "$PR_BASE_SHA")"

if ! changed_paths="$(git diff --name-only "$merge_base" "$PR_HEAD_SHA")"; then
  echo "::error::compute-pr-job-gates: git diff $merge_base..$PR_HEAD_SHA failed" >&2
  exit 1
fi
while IFS= read -r path; do
  classify_path "$path"
done <<<"$changed_paths"

# Latest commit on the PR branch (covers speech-only pushes after merge-base is stale).
if git rev-parse --verify "${PR_HEAD_SHA}^{commit}" >/dev/null 2>&1; then
  if ! head_paths="$(git diff-tree --no-commit-id --name-only -r "$PR_HEAD_SHA")"; then
    echo "::error::compute-pr-job-gates: git diff-tree $PR_HEAD_SHA failed" >&2
    exit 1
  fi
  while IFS= read -r path; do
    classify_path "$path"
  done <<<"$head_paths"
fi

# Post-release lock PRs only bump our own @node-webrtc-rust/* versions: quality + build-ts only.
version_only=false
if [[ "$run_test" == true ]] && command -v python3 >/dev/null 2>&1; then
  if [[ "$(python3 -I "$SCRIPT_DIR/version-only-diff.py" "$merge_base" "$PR_HEAD_SHA" 2>/dev/null || echo false)" == true ]]; then
    version_only=true
    run_compile=false
    run_test=false
    run_build_ts=true
    run_quality=true
  fi
fi

# Union of suites over the changed code paths, from pr-test-suites.map. Fails closed (exit 1) on a
# missing file or a malformed rule line.
compute_test_suites() {
  local map="$SCRIPT_DIR/pr-test-suites.map"
  local globs=() suites=()
  local line glob rule_suites extra s parts
  if [[ ! -f "$map" ]]; then
    echo "compute-pr-job-gates: missing suite map $map" >&2
    exit 1
  fi
  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line#"${line%%[![:space:]]*}"}"
    [[ -z "$line" || "$line" == \#* ]] && continue
    glob="" rule_suites="" extra=""
    read -r glob rule_suites extra <<<"$line"
    if [[ -z "$glob" || -z "$rule_suites" || -n "$extra" ]]; then
      echo "compute-pr-job-gates: malformed rule in $map (want '<glob> <suites>'): $line" >&2
      exit 1
    fi
    IFS=',' read -ra parts <<<"$rule_suites"
    for s in "${parts[@]}"; do
      case "$s" in
        rust | node | sherpa) ;;
        *)
          echo "compute-pr-job-gates: unknown suite '$s' in $map: $line" >&2
          exit 1
          ;;
      esac
    done
    globs+=("$glob")
    suites+=("$rule_suites")
  done <"$map"

  local want_rust=false want_node=false want_sherpa=false
  local path i matched rs
  for path in "${code_paths[@]+"${code_paths[@]}"}"; do
    matched="rust,node,sherpa"
    for i in "${!globs[@]}"; do
      # shellcheck disable=SC2053
      if [[ "$path" == ${globs[$i]} ]]; then
        matched="${suites[$i]}"
        break
      fi
    done
    rs=",$matched,"
    [[ "$rs" == *",rust,"* ]] && want_rust=true
    [[ "$rs" == *",node,"* ]] && want_node=true
    [[ "$rs" == *",sherpa,"* ]] && want_sherpa=true
  done
  local out=""
  [[ "$want_rust" == true ]] && out="rust"
  [[ "$want_node" == true ]] && out="${out:+$out,}node"
  [[ "$want_sherpa" == true ]] && out="${out:+$out,}sherpa"
  echo "$out"
}

test_suites=""
if [[ "$run_test" == true ]]; then
  test_suites="$(compute_test_suites)"
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
emit_bool version_only "$version_only"
echo "test_suites=${test_suites}"

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

echo "::notice::PR gates merge_base=${merge_base:0:12} head=${PR_HEAD_SHA:0:12} — compile=${run_compile} build_ts=${run_build_ts} test=${run_test} quality=${run_quality} version_only=${version_only} test_suites=${test_suites}"
