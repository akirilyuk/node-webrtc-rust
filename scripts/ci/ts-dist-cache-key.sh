#!/usr/bin/env bash
# Fingerprint for packages/sdk, signaling, helpers dist/ — must match
# .github/actions/ci-cache-ts-dist/action.yml hashFiles() inputs (+ Node major
# appended by the action / ensure-ts-dist via TS_DIST_NODE_MAJOR).
#
# Uses find(1) only (works in ci-build container where .git is absent).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

NODE_MAJOR="${TS_DIST_NODE_MAJOR:-20}"

{
  printf 'node-major=%s\n' "$NODE_MAJOR"
  printf 'build-contract=workspace-ts-dist\n'

  # Package manifests + lock (compiler identity lives in lockfile)
  printf '%s\n' package.json package-lock.json
  printf '%s\n' \
    packages/sdk/package.json \
    packages/signaling/package.json \
    packages/helpers/package.json

  # Build / stamp scripts
  printf '%s\n' \
    scripts/ci/build-ts-workspace.sh \
    scripts/ci/ts-dist-cache-key.sh \
    scripts/ci/ensure-ts-dist.sh \
    scripts/ci/tsconfig.build-sdk.cjs.json \
    scripts/ci/tsconfig.build-sdk.esm.json

  # Sources
  find packages/sdk/src packages/signaling/src packages/helpers/src -type f 2>/dev/null || true

  # Package + base tsconfigs
  find packages/sdk packages/signaling packages/helpers -maxdepth 1 -name 'tsconfig*.json' -type f 2>/dev/null || true
  [[ -f tsconfig.base.json ]] && printf '%s\n' tsconfig.base.json
} | LC_ALL=C sort -u | while IFS= read -r path; do
  if [[ "$path" == node-major=* || "$path" == build-contract=* ]]; then
    printf '%s\0' "$path"
    continue
  fi
  [[ -n "$path" && -f "$path" ]] || continue
  printf '%s\0' "$path"
  cat "$path"
done | shasum -a 256 | awk '{print $1}'
