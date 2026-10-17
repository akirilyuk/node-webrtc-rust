#!/usr/bin/env bash
# Guards scripts/ci/pr-test-suites.map (error type test.unscheduled-suite-rot: a suite that a
# change can break but that is never selected for it).
#   1. Every directory under crates/, packages/, examples/ and third_party/ has an explicit
#      '<dir>/*' rule, so a new crate/package/example cannot silently fall through to a guess.
#   2. Every rule glob matches at least one tracked file (no stale rules), and every suite name is
#      rust, node or sherpa.
#   3. main and release never pass test_suites: they always run every suite.
# The check bites: run against a map without crates/core/* it must fail.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

MAP="scripts/ci/pr-test-suites.map"

# check_map <map-file>: prints every problem to stderr; returns 1 when there is one.
check_map() {
  local map="$1" problems=0
  local globs=() line glob suites extra s parts

  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line#"${line%%[![:space:]]*}"}"
    [[ -z "$line" || "$line" == \#* ]] && continue
    glob="" suites="" extra=""
    read -r glob suites extra <<<"$line"
    if [[ -z "$glob" || -z "$suites" || -n "$extra" ]]; then
      echo "malformed rule (want '<glob> <suites>'): $line" >&2
      problems=1
      continue
    fi
    IFS=',' read -ra parts <<<"$suites"
    for s in "${parts[@]}"; do
      case "$s" in
        rust | node | sherpa) ;;
        *)
          echo "unknown suite '$s' in rule: $line" >&2
          problems=1
          ;;
      esac
    done
    globs+=("$glob")
  done <"$map"

  local files dirs path
  files="$(git ls-files -- crates packages examples third_party)"
  dirs="$(printf '%s\n' "$files" | awk -F/ 'NF >= 3 { print $1 "/" $2 }' | sort -u)"

  local d found g
  while IFS= read -r d; do
    [[ -z "$d" ]] && continue
    found=false
    for g in "${globs[@]+"${globs[@]}"}"; do
      if [[ "$g" == "$d/*" ]]; then
        found=true
        break
      fi
    done
    if [[ "$found" != true ]]; then
      echo "no '$d/*' rule in $map (add one: which of rust,node,sherpa can this directory affect?)" >&2
      problems=1
    fi
  done <<<"$dirs"

  for g in "${globs[@]+"${globs[@]}"}"; do
    found=false
    while IFS= read -r path; do
      # shellcheck disable=SC2053
      if [[ "$path" == $g ]]; then
        found=true
        break
      fi
    done <<<"$files"
    if [[ "$found" != true ]]; then
      echo "stale rule '$g' in $map: matches no tracked file" >&2
      problems=1
    fi
  done

  return "$problems"
}

if ! check_map "$MAP"; then
  exit 1
fi

# Self-test: a map without crates/core/* must be rejected.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
grep -v '^crates/core/\*' "$MAP" >"$tmp/missing-core.map"
if grep -q '^crates/core/\*' "$tmp/missing-core.map"; then
  echo "self-test setup failed: crates/core/* still in the temp map" >&2
  exit 1
fi
if check_map "$tmp/missing-core.map" 2>/dev/null; then
  echo "self-test failed: a map without crates/core/* was accepted" >&2
  exit 1
fi

# Stale-rule self-test.
{
  cat "$MAP"
  echo "crates/does-not-exist/* rust"
} >"$tmp/stale.map"
if check_map "$tmp/stale.map" 2>/dev/null; then
  echo "self-test failed: a map with a stale rule was accepted" >&2
  exit 1
fi

# main and release always run every suite.
for wf in .github/workflows/build-main.yml .github/workflows/release.yml; do
  if grep -Eq '^[[:space:]]*test_suites:' "$wf"; then
    echo "$wf: must not pass test_suites (main and release run every suite)" >&2
    exit 1
  fi
done

echo "pr-test-suites-map.test.sh: OK"
