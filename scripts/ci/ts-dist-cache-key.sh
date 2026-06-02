#!/usr/bin/env bash
# Fingerprint for packages/sdk, signaling, helpers dist/ — must match
# .github/actions/ci-cache-ts-dist/action.yml hashFiles() inputs.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "ts-dist-cache-key: not a git repo" >&2
  exit 1
fi

{
  git ls-files \
    'packages/sdk/src' \
    'packages/signaling/src' \
    'packages/helpers/src' \
    'packages/sdk/tsconfig' \
    'packages/signaling/tsconfig' \
    'packages/helpers/tsconfig' \
    'tsconfig.base.json' 2>/dev/null || true
} | LC_ALL=C sort | while IFS= read -r path; do
  [[ -n "$path" && -f "$path" ]] || continue
  printf '%s\0' "$path"
  cat "$path"
done | shasum -a 256 | awk '{print $1}'
