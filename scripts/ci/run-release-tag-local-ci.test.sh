#!/usr/bin/env bash
# Guards the release-tag gate order: build:native -> check-native-exports -> quality -> tests-full.
# Native first so local tests never load a stale .node (0.9.42 incident). The check must NOT be
# in run-pr-quality.sh: the Release quality job uses the previous published binding on purpose.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

GATE="$ROOT/scripts/ci/run-release-tag-local-ci.sh"

line_of() {
  local pattern="$1" file="$2" n
  n="$(grep -n -m1 -E "$pattern" "$file" | cut -d: -f1 || true)"
  if [[ -z "$n" ]]; then
    echo "missing pattern in $file: $pattern" >&2
    return 1
  fi
  echo "$n"
}

assert_order() {
  local file="$1" b e q t
  b="$(line_of '^run_step "build:native"' "$file")" || return 1
  e="$(line_of '^run_step "check-native-exports"' "$file")" || return 1
  q="$(line_of '^run_step "run-pr-quality.sh"' "$file")" || return 1
  t="$(line_of '^run_step "run-pr-tests-full.sh"' "$file")" || return 1
  if ! (( b < e && e < q && q < t )); then
    echo "wrong order in $file: build:native=$b check-native-exports=$e quality=$q tests-full=$t" >&2
    return 1
  fi
}

assert_order "$GATE"

if grep -n 'check-native-exports' scripts/ci/run-pr-quality.sh; then
  echo "run-pr-quality.sh must not run check-native-exports (Release quality uses the published binding)" >&2
  exit 1
fi

# The guard must bite: quality moved before build:native has to fail.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
bad="$tmp/bad-gate.sh"
awk '
  /^run_step "build:native"/ { held[++n] = $0; next }
  /^run_step "check-native-exports"/ { held[++n] = $0; next }
  /^run_step "run-pr-quality.sh"/ { print; for (i = 1; i <= n; i++) print held[i]; next }
  { print }
' "$GATE" >"$bad"
if assert_order "$bad" 2>/dev/null; then
  echo "guard did not detect quality-before-native order" >&2
  exit 1
fi

echo "OK: run-release-tag-local-ci order test"
