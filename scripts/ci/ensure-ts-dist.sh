#!/usr/bin/env bash
# Ensure sdk/signaling/helpers dist/ matches current TS sources.
#
# GHA cache restore-keys can leave a partial dist/ that still has index.js but is
# stale. We stamp the exact source fingerprint after every build (see build-ts-workspace.sh).
#
# Env:
#   TS_DIST_CACHE_HIT=true  — exact GHA cache key match; skip rebuild if stamp OK
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

STAMP_FILE="packages/sdk/dist/.ci-ts-dist-key"
REQUIRED=(
  packages/sdk/dist/cjs/index.js
  packages/signaling/dist/cjs/index.js
  packages/helpers/dist/cjs/index.js
)

KEY="$(bash "$ROOT/scripts/ci/ts-dist-cache-key.sh")"

dist_ok=true
for f in "${REQUIRED[@]}"; do
  if [[ ! -f "$f" ]]; then
    dist_ok=false
    break
  fi
done

stamp_ok=false
if [[ -f "$STAMP_FILE" ]] && [[ "$(cat "$STAMP_FILE")" == "$KEY" ]]; then
  stamp_ok=true
fi

if [[ "${TS_DIST_CACHE_HIT:-}" == "true" ]] && "$dist_ok" && "$stamp_ok"; then
  echo "==> build:ts skipped (dist cache exact hit, stamp ${KEY:0:12}…)"
  exit 0
fi

if "$dist_ok" && "$stamp_ok"; then
  echo "==> build:ts skipped (dist matches sources, stamp ${KEY:0:12}…)"
  exit 0
fi

if ! "$dist_ok"; then
  echo "==> build:ts (dist artifacts missing)"
elif ! "$stamp_ok"; then
  echo "==> build:ts (TS sources changed — stamp mismatch or missing)"
else
  echo "==> build:ts (CI cache partial hit — rebuild for exact sources)"
fi

bash "$ROOT/scripts/ci/build-ts-workspace.sh"
