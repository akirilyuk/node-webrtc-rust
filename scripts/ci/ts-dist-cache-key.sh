#!/usr/bin/env bash
# Fingerprint for packages/sdk, signaling, helpers dist/ — must match
# .github/actions/ci-cache-ts-dist/action.yml hashFiles() inputs.
#
# Uses find(1) only (works in ci-build container where .git is absent).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

{
  find packages/sdk/src packages/signaling/src packages/helpers/src -type f 2>/dev/null || true
  find packages/sdk packages/signaling packages/helpers -maxdepth 1 -name 'tsconfig*.json' -type f 2>/dev/null || true
  [[ -f tsconfig.base.json ]] && printf '%s\n' tsconfig.base.json
} | LC_ALL=C sort -u | while IFS= read -r path; do
  [[ -n "$path" && -f "$path" ]] || continue
  printf '%s\0' "$path"
  cat "$path"
done | shasum -a 256 | awk '{print $1}'
