#!/usr/bin/env bash
# Write .test-logs/release-tag-gate/<tested sha>.json after local release-tag CI.
# Do not call this unless quality, native, and tests-full exited 0 for this SHA.
# The tested SHA is a required argument: the stamp names the SHA the gate ran on, never a
# HEAD re-read at the end. If HEAD moved during the run, this refuses and writes nothing.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

TESTED_SHA="${1:-}"
QUALITY="${2:-}"
NATIVE="${3:-}"
TESTS="${4:-}"

if [[ -z "$TESTED_SHA" || -z "$QUALITY" || -z "$NATIVE" || -z "$TESTS" ]]; then
  echo "usage: write-release-tag-gate.sh <tested-sha> <quality-exit> <native-exit> <tests-exit>" >&2
  echo 'example: bash scripts/ci/write-release-tag-gate.sh "$(git rev-parse HEAD)" 0 0 0' >&2
  exit 2
fi
if [[ ! "$TESTED_SHA" =~ ^[0-9a-f]{40}$ ]]; then
  echo "write-release-tag-gate: <tested-sha> must be a full 40-hex SHA (got ${TESTED_SHA})" >&2
  exit 2
fi

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
if [[ "$BRANCH" != "main" ]]; then
  echo "write-release-tag-gate: must be on main (got ${BRANCH})" >&2
  exit 1
fi

SHA="$(git rev-parse HEAD)"
if [[ "$SHA" != "$TESTED_SHA" ]]; then
  echo "write-release-tag-gate: HEAD moved during the gate run (tested ${TESTED_SHA}, now ${SHA})" >&2
  exit 1
fi
ORIGIN_MAIN="$(git rev-parse origin/main 2>/dev/null || true)"
if [[ -z "$ORIGIN_MAIN" ]]; then
  echo "write-release-tag-gate: fetch origin/main first" >&2
  exit 1
fi
if [[ "$SHA" != "$ORIGIN_MAIN" ]]; then
  echo "write-release-tag-gate: HEAD (${SHA}) != origin/main (${ORIGIN_MAIN})" >&2
  exit 1
fi

if [[ "$QUALITY" != "0" || "$NATIVE" != "0" || "$TESTS" != "0" ]]; then
  echo "write-release-tag-gate: refusing stamp (quality=${QUALITY} native=${NATIVE} tests=${TESTS})" >&2
  exit 1
fi

DIR="$ROOT/.test-logs/release-tag-gate"
mkdir -p "$DIR"
python3 - "$DIR/$SHA.json" "$SHA" "$QUALITY" "$NATIVE" "$TESTS" <<'PY'
import json, sys, time
path, sha, quality, native, tests = sys.argv[1:]
payload = {
    "sha": sha,
    "quality": int(quality),
    "native": int(native),
    "tests": int(tests),
    "created_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
}
with open(path, "w", encoding="utf-8") as f:
    json.dump(payload, f, indent=2)
    f.write("\n")
print(path)
PY
