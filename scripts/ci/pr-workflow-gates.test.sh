#!/usr/bin/env bash
# Guards two PR-workflow properties:
#   1. ci.path-filter-negation-or: under dorny/paths-filter's default OR quantifier a '!' pattern
#      matched every file and forced the full pipeline on docs PRs. Any filter with a '!' pattern
#      needs `predicate-quantifier: every`.
#   2. The build.yml concurrency group is keyed on the branch, not the PR number, so a
#      workflow_dispatch run and a pull_request run of the same branch cancel each other.
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

echo "pr-workflow-gates.test.sh: OK"
