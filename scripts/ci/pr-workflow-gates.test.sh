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
# build.yml: the only allowed 'docker run' is the host permission reset in steps named
# 'Prepare workspace' (host jobs, not the integration job). Any other step must not use Docker.
if awk '
  /^      - / { in_prepare = ($0 ~ /name: Prepare workspace/) }
  /^[^#]*docker run/ && !in_prepare { print FILENAME ":" NR ": " $0; found = 1 }
  END { exit found ? 0 : 1 }
' "$build" >&2; then
  echo "$build: 'docker run' outside a 'Prepare workspace' step (Kubernetes runners have no Docker daemon)" >&2
  exit 1
fi
integration="scripts/ci/run-pr-integration.sh"
if ! grep -Eq 'turnserver-ci\.sh"? start' "$integration"; then
  echo "$integration: must call turnserver-ci.sh start (coturn runs inside the ci-test container)" >&2
  exit 1
fi
if ! grep -Eq "trap '.*turnserver-ci\.sh\"? stop.*' EXIT" "$integration"; then
  echo "$integration: must install a trap that calls turnserver-ci.sh stop" >&2
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
