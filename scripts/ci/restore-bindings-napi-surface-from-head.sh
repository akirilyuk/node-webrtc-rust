#!/usr/bin/env bash
# Restore committed NAPI JS/TS surface after `napi build`.
#
# `npx napi build` rewrites packages/bindings/index.js (and sometimes
# index.d.ts). Producer manifests must hash the **published** surface (HEAD)
# so Assemble native main bundle / release validation match. Run after
# build-native-addon.sh and before verify-native-binding-surface /
# write-native-artifact-manifest — not inside the fingerprinted compile recipe.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

SURFACE=(
  packages/bindings/index.js
  packages/bindings/index.d.ts
)

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "restore-bindings-napi-surface-from-head: not a git work tree at $ROOT" >&2
  exit 1
fi

if git diff --quiet -- "${SURFACE[@]}"; then
  echo "ok: bindings napi surface already matches HEAD"
  exit 0
fi

echo "notice: napi build drifted committed surface; restoring from HEAD for manifest/verify"
git --no-pager diff --stat -- "${SURFACE[@]}" || true
git checkout HEAD -- "${SURFACE[@]}"
echo "ok: restored ${SURFACE[*]} from HEAD"
