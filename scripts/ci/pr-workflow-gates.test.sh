#!/usr/bin/env bash
# Guards two PR-workflow properties:
#   1. ci.path-filter-negation-or: under dorny/paths-filter's default OR quantifier a '!' pattern
#      matched every file and forced the full pipeline on docs PRs. Any filter with a '!' pattern
#      needs `predicate-quantifier: every`.
#   2. The build.yml concurrency group is keyed on the branch, not the PR number, so a
#      workflow_dispatch run and a pull_request run of the same branch cancel each other.
#   3. The integration job runs in the ci-test container with coturn started in-process: the
#      container image and TURN_AVAILABLE are set (turn.test.ts cannot silently skip), no job step
#      calls `docker run` (Kubernetes runner pods have no Docker daemon), and run-pr-integration.sh
#      starts and stops turnserver itself.
#   4. Path-based suite selection (CI_TEST_SUITES / test_suites): empty means every suite.
#   5. ARC runners, all workflows (allowlists in the block below): no plain `runs-on: self-hosted`,
#      no docker commands, every NWR_RUNNER_* job also has container:, and every workflow that uses
#      NWR_RUNNER_* defaults run: steps to bash and sets the wildcard safe.directory through
#      GIT_CONFIG_* env. Nothing under .github mentions CARGO_VIA_CI_IMAGE. Every runs-on in build.yml
#      and reusable-test.yml uses the NWR_RUNNER_* variables.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

check_paths_filter_file() {
  local f="$1"
  if grep -q 'dorny/paths-filter' "$f" \
    && grep -Eq "^[[:space:]]*-[[:space:]]*['\"]?!" "$f" \
    && ! grep -Eq "predicate-quantifier:[[:space:]]*['\"]?every['\"]?" "$f"; then
    echo "$f: dorny/paths-filter with a '!' pattern needs predicate-quantifier: every (ci.path-filter-negation-or)" >&2
    return 1
  fi
  return 0
}

failed=0
shopt -s nullglob
for f in .github/workflows/*.yml .github/actions/*/action.yml; do
  check_paths_filter_file "$f" || failed=1
done
if [ "$failed" -ne 0 ]; then
  exit 1
fi

# Self-test: the check must bite.
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
cat >"$tmp" <<'YAML'
    - uses: dorny/paths-filter@v3
      with:
        filters: |
          native:
            - 'crates/**'
            - '!**/*.md'
YAML
if check_paths_filter_file "$tmp" 2>/dev/null; then
  echo "self-test failed: negation filter without predicate-quantifier was accepted" >&2
  exit 1
fi
echo "        predicate-quantifier: 'every'" >>"$tmp"
if ! check_paths_filter_file "$tmp"; then
  echo "self-test failed: negation filter with predicate-quantifier: every was rejected" >&2
  exit 1
fi

# Concurrency key.
build=".github/workflows/build.yml"
if ! grep -Eq '^[[:space:]]*group:.*github\.head_ref \|\| github\.ref_name' "$build"; then
  echo "$build: concurrency group must be keyed on 'github.head_ref || github.ref_name' (one run per branch)" >&2
  exit 1
fi
if grep -q 'pull_request.number' "$build"; then
  echo "$build: must not key concurrency on pull_request.number (dispatched and PR runs would not collapse)" >&2
  exit 1
fi

# Integration job: ci-test container + in-process coturn.
reusable=".github/workflows/reusable-test.yml"
if ! grep -Fq 'image: ghcr.io/${{ github.repository }}/ci-build:test' "$reusable"; then
  echo "$reusable: the test job must run in container image ghcr.io/\${{ github.repository }}/ci-build:test (Dockerfile.test)" >&2
  exit 1
fi
if ! grep -Eq "^[[:space:]]*TURN_AVAILABLE:[[:space:]]*'1'" "$reusable"; then
  echo "$reusable: must set TURN_AVAILABLE: '1' (packages/sdk/tests/turn.test.ts skips silently without it)" >&2
  exit 1
fi
if grep -Eq '^[^#]*docker run' "$reusable"; then
  echo "$reusable: must not call 'docker run' (Kubernetes runners have no Docker daemon)" >&2
  exit 1
fi
# ARC migration guards over every workflow (plan: 2026-10-20-nwr-ci-arc-migration.md):
#   a. no plain `runs-on: self-hosted`. vendor-bench.yml picks its runner by an input (`location`:
#      eu = the one old self-hosted EU runner kept for benchmarks, th = nwr-large), so it has no plain
#      line either; the allowlist below stays empty unless a workflow must pin the EU runner;
#   b. no docker commands (ARC runner pods have no Docker daemon). ci-image.yml builds images on
#      GitHub-hosted ubuntu-latest through docker/* actions (`uses:` lines), so it is exempt;
#   c. every job that runs on NWR_RUNNER_* also has container:, so the NWR_RUNNER_* fallback to
#      self-hosted runs the same YAML;
#   d. every workflow that uses NWR_RUNNER_* sets a top-level bash default shell and the wildcard
#      safe.directory env (container-mode run: steps default to `sh -e`; the job pod runs as root on
#      a workspace owned by another user);
#   e. nothing under .github mentions CARGO_VIA_CI_IMAGE (cargo is on PATH inside ci-build).
# Allowlist for (a): none (vendor-bench chooses self-hosted only through its location input).
SELF_HOSTED_ALLOWLIST=""
# Allowlist for (b): builds images with docker/* actions on ubuntu-latest.
DOCKER_ALLOWLIST="ci-image.yml"
# Allowlist for (d): the vendor-live-* jobs set `shell: bash` on the steps that need it and run no git
# commands, so they carry no workflow-level defaults.
DEFAULTS_ALLOWLIST_GLOB="vendor-live-*.yml"

in_allowlist() {
  local name="$1" list="$2" item
  for item in $list; do
    [ "$item" = "$name" ] && return 0
  done
  return 1
}

# Non-comment lines of a workflow.
code_lines() { grep -Ev '^[[:space:]]*#' "$1" || true; }

check_no_plain_self_hosted() {
  if code_lines "$1" | grep -Eq '^[[:space:]]*runs-on:[[:space:]]*self-hosted[[:space:]]*$'; then
    echo "$1: plain 'runs-on: self-hosted'; use \${{ vars.NWR_RUNNER_SMALL || 'nwr-small' }} (or _LARGE) with a container: block" >&2
    return 1
  fi
  return 0
}

check_no_docker_commands() {
  if code_lines "$1" | grep -Ewn 'docker' >&2; then
    echo "$1: must not contain docker commands (docker run/build/login); ARC runners have no Docker daemon" >&2
    return 1
  fi
  return 0
}

# Prints the jobs that run on NWR_RUNNER_* (or any runs-on in $2=all) but have no container: block.
# Jobs that call a reusable workflow (uses:) have no runs-on and are skipped; GitHub-hosted jobs are
# skipped.
jobs_missing_container() {
  awk '
    function flush() { if (job != "" && has_nwr && !has_container) print job }
    /^jobs:/ { in_jobs = 1; next }
    in_jobs && /^[^[:space:]#]/ { flush(); job = ""; in_jobs = 0 }
    in_jobs && /^  [A-Za-z0-9_-]+:[[:space:]]*$/ {
      flush()
      job = $1; sub(/:$/, "", job); has_nwr = 0; has_container = 0; next
    }
    in_jobs && /^    runs-on:.*NWR_RUNNER_/ { has_nwr = 1 }
    in_jobs && /^    container:/ { has_container = 1 }
    END { flush() }
  ' "$1"
}

check_nwr_jobs_have_container() {
  local missing
  missing="$(jobs_missing_container "$1")"
  if [ -n "$missing" ]; then
    echo "$1: NWR_RUNNER_* jobs without a container: block: $(echo "$missing" | tr '\n' ' ')" >&2
    echo "$1: every Linux job is a container: job so the NWR_RUNNER_* fallback to self-hosted keeps working" >&2
    return 1
  fi
  return 0
}

check_bash_default_and_safe_directory() {
  if ! awk '
    /^defaults:/ { in_defaults = 1; next }
    in_defaults && /^[^[:space:]#]/ { in_defaults = 0 }
    in_defaults && /^    shell:[[:space:]]*bash[[:space:]]*$/ { found = 1 }
    END { exit found ? 0 : 1 }
  ' "$1"; then
    echo "$1: must set 'shell: bash' under a top-level defaults.run block (container-mode run: steps default to sh -e)" >&2
    return 1
  fi
  if ! grep -Eq '^[[:space:]]*GIT_CONFIG_KEY_0:[[:space:]]*safe\.directory[[:space:]]*$' "$1"; then
    echo "$1: must set GIT_CONFIG_KEY_0: safe.directory (with GIT_CONFIG_COUNT/VALUE_0) so git works on the root-owned-vs-runner workspace" >&2
    return 1
  fi
  return 0
}

for wf in .github/workflows/*.yml; do
  name="$(basename "$wf")"
  if ! in_allowlist "$name" "$SELF_HOSTED_ALLOWLIST"; then
    check_no_plain_self_hosted "$wf" || exit 1
  fi
  if ! in_allowlist "$name" "$DOCKER_ALLOWLIST"; then
    check_no_docker_commands "$wf" || exit 1
  fi
  check_nwr_jobs_have_container "$wf" || exit 1
  if grep -q 'NWR_RUNNER_' "$wf"; then
    # shellcheck disable=SC2254
    case "$name" in
      $DEFAULTS_ALLOWLIST_GLOB) ;;
      *) check_bash_default_and_safe_directory "$wf" || exit 1 ;;
    esac
  fi
done
if grep -rn 'CARGO_VIA_CI_IMAGE' .github >&2; then
  echo ".github: CARGO_VIA_CI_IMAGE must not appear (cargo is on PATH inside the ci-build container)" >&2
  exit 1
fi

# Self-tests: each guard must bite on a minimal YAML and accept its good counterpart.
tmp_jobs="$(mktemp)"
trap 'rm -f "$tmp" "$tmp_jobs"' EXIT
cat >"$tmp_jobs" <<'YAML'
jobs:
  with-container:
    runs-on: ${{ vars.NWR_RUNNER_SMALL || 'nwr-small' }}
    container:
      image: example
    steps:
      - run: true
  no-container:
    runs-on: ${{ vars.NWR_RUNNER_SMALL || 'nwr-small' }}
    steps:
      - run: true
  github-hosted:
    runs-on: ubuntu-latest
    steps:
      - run: true
  reusable:
    uses: ./.github/workflows/reusable-test.yml
YAML
if [ "$(jobs_missing_container "$tmp_jobs")" != "no-container" ]; then
  echo "self-test failed: jobs_missing_container did not report exactly the NWR job without container:" >&2
  exit 1
fi
if check_nwr_jobs_have_container "$tmp_jobs" 2>/dev/null; then
  echo "self-test failed: an NWR job without container: was accepted" >&2
  exit 1
fi
cat >"$tmp_jobs" <<'YAML'
jobs:
  old-runner:
    runs-on: self-hosted
    steps:
      - run: true
YAML
if check_no_plain_self_hosted "$tmp_jobs" 2>/dev/null; then
  echo "self-test failed: a plain runs-on: self-hosted job was accepted" >&2
  exit 1
fi
cat >"$tmp_jobs" <<'YAML'
jobs:
  old-runner:
    # runs-on: self-hosted (comment only)
    runs-on: ubuntu-latest
    steps:
      # docker run is mentioned in a comment only
      - uses: docker/setup-buildx-action@v3
YAML
if ! check_no_plain_self_hosted "$tmp_jobs" 2>/dev/null; then
  echo "self-test failed: a commented-out self-hosted line was rejected" >&2
  exit 1
fi
cat >"$tmp_jobs" <<'YAML'
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: docker run --rm alpine true
YAML
if check_no_docker_commands "$tmp_jobs" 2>/dev/null; then
  echo "self-test failed: a docker run command was accepted" >&2
  exit 1
fi
cat >"$tmp_jobs" <<'YAML'
name: x
env:
  A: b
jobs: {}
YAML
if check_bash_default_and_safe_directory "$tmp_jobs" 2>/dev/null; then
  echo "self-test failed: a workflow without defaults.run.shell: bash was accepted" >&2
  exit 1
fi

# Runners in build.yml and reusable-test.yml come from the NWR_RUNNER_* variables with an ARC
# default, never a hard-coded runner label.
for wf in "$build" "$reusable"; do
  if grep -E '^[[:space:]]*runs-on:' "$wf" | grep -Fv 'NWR_RUNNER_' >&2; then
    echo "$wf: every runs-on must use the NWR_RUNNER_* repo variables with an ARC default (no plain self-hosted)" >&2
    exit 1
  fi
done
integration="scripts/ci/run-pr-integration.sh"
if ! grep -Eq 'turnserver-ci\.sh"? start' "$integration"; then
  echo "$integration: must call turnserver-ci.sh start (coturn runs inside the ci-test container)" >&2
  exit 1
fi
if ! grep -Eq "trap '.*turnserver-ci\.sh\"? stop.*' EXIT" "$integration"; then
  echo "$integration: must install a trap that calls turnserver-ci.sh stop" >&2
  exit 1
fi

# Path-based suite selection: empty CI_TEST_SUITES means every suite, and the PR workflow passes the
# computed list through the reusable workflow's test_suites input (default '' = all).
if ! grep -Eq '^suite_enabled\(\)' "$integration" || ! grep -Fq -- '-z "$CI_TEST_SUITES"' "$integration"; then
  echo "$integration: must define suite_enabled() with the empty-means-all rule (-z \"\$CI_TEST_SUITES\")" >&2
  exit 1
fi
if ! grep -Fq 'CI_TEST_SUITES: ${{ inputs.test_suites }}' "$reusable"; then
  echo "$reusable: must pass CI_TEST_SUITES: \${{ inputs.test_suites }} to the test script" >&2
  exit 1
fi
if ! awk '
  /^      test_suites:/ { in_input = 1; next }
  in_input && /^        default:/ { print; exit }
  in_input && /^      [a-z_]+:/ { exit }
' "$reusable" | grep -Eq "default:[[:space:]]*''"; then
  echo "$reusable: input test_suites must default to '' (empty = all suites)" >&2
  exit 1
fi

# run-ci-shell-tests.sh discovers tests by glob; the only test names in it are SKIP entries
# (ci.local-gate-not-in-ci: a hand-kept list lets a new *.test.sh stay local-only).
runner="scripts/ci/run-ci-shell-tests.sh"
if ! grep -Fq 'scripts/ci/*.test.sh' "$runner" || ! grep -Eq 'for f in scripts/ci/\*\.test\.sh' "$runner"; then
  echo "$runner: must discover tests with 'for f in scripts/ci/*.test.sh'" >&2
  exit 1
fi
if grep -En '[A-Za-z0-9_-]+\.test\.sh' "$runner" | grep -Ev '^[0-9]+:[[:space:]]*(#|"[A-Za-z0-9_-]+\.test\.sh\|)' | grep -Ev '\*\.test\.sh|\$name|\.test\.sh\)|scripts/ci/\$' >/dev/null; then
  echo "$runner: names a test outside the SKIP map; tests are discovered by glob:" >&2
  grep -En '[A-Za-z0-9_-]+\.test\.sh' "$runner" | grep -Ev '^[0-9]+:[[:space:]]*(#|"[A-Za-z0-9_-]+\.test\.sh\|)' | grep -Ev '\*\.test\.sh|\$name|\.test\.sh\)|scripts/ci/\$' >&2
  exit 1
fi

echo "pr-workflow-gates.test.sh: OK"
