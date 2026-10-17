#!/usr/bin/env bash
# Run every scripts/ci/*.test.sh (discovered by glob, sorted) except the reviewed SKIP list below.
# Runs all tests, then reports the failing names and exits 1 if any failed.
#
# Why: a shell test that only `verify-checks.sh` ran locally broke unnoticed on main
# (ci.local-gate-not-in-ci). The PR quality job (run-pr-quality.sh) and verify-checks.sh both call
# this script, so a new *.test.sh is gated in CI the moment it is added. Do not list tests by name
# anywhere else; add a SKIP entry here, with a concrete reason, only for a test that cannot run on
# the quality host (self-hosted Linux, Node 20, git, python3; no Rust toolchain, no ripgrep: use grep).
#
# SKIP entries are "<test file name>|<reason>". A stale entry (file missing) fails the run.
# (A plain array, not an associative one: macOS /bin/bash is 3.2.)
set -uo pipefail

SKIP=(
  "native-artifact-bundle.test.sh|needs a Rust toolchain: the bundle script runs native_build_contract.py (cargo metadata)"
  "native-artifact-manifest.test.sh|needs a Rust toolchain: native_build_contract.py runs cargo metadata"
  "native-binding-cache-key.test.sh|needs a Rust toolchain: native_build_contract.py runs cargo metadata"
  "native-build-fingerprint.test.sh|needs a Rust toolchain: native_build_contract.py runs cargo metadata"
  "resolve-native-main-bundle.test.sh|needs a Rust toolchain: native_build_contract.py runs cargo metadata"
)

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

skip_reason() {
  local entry
  for entry in "${SKIP[@]}"; do
    if [[ "${entry%%|*}" == "$1" ]]; then
      printf '%s' "${entry#*|}"
      return 0
    fi
  done
  return 1
}

stale=0
for entry in "${SKIP[@]}"; do
  name="${entry%%|*}"
  if [[ ! -f "scripts/ci/$name" ]]; then
    echo "stale SKIP entry (no scripts/ci/$name): $name" >&2
    stale=1
  fi
done
[[ "$stale" -eq 0 ]] || exit 1

failed=()
ran=0
for f in scripts/ci/*.test.sh; do
  name="$(basename "$f")"
  if reason="$(skip_reason "$name")"; then
    echo "skip: $name ($reason)"
    continue
  fi
  echo "==> $name"
  ran=$((ran + 1))
  if ! bash "$f"; then
    failed+=("$name")
  fi
done

if [[ ${#failed[@]} -gt 0 ]]; then
  echo "FAILED CI shell tests (${#failed[@]}/$ran):" >&2
  printf '  %s\n' "${failed[@]}" >&2
  exit 1
fi
echo "run-ci-shell-tests.sh: OK ($ran ran)"
