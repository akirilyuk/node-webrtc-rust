#!/usr/bin/env bash
# Guards that run-pr-quality.sh runs the repo-wide Prettier check (npm run format:check)
# after lint, so formatting drift fails the PR and the Release quality job.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

GATE="$ROOT/scripts/ci/run-pr-quality.sh"

line_of() {
  local pattern="$1" file="$2" n
  n="$(grep -n -m1 -E "$pattern" "$file" | cut -d: -f1 || true)"
  if [[ -z "$n" ]]; then
    echo "missing pattern in $file: $pattern" >&2
    return 1
  fi
  echo "$n"
}

lint="$(line_of '^npm run lint$' "$GATE")"
fmt="$(line_of '^npm run format:check$' "$GATE")"
if ! (( lint < fmt )); then
  echo "wrong order in $GATE: npm run lint=$lint npm run format:check=$fmt" >&2
  exit 1
fi

echo "run-pr-quality.test.sh: OK"
