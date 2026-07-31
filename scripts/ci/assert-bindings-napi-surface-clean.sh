#!/usr/bin/env bash
# Fail closed when napi build rewrites committed JS/TS surface files.
#
# napi build may reorder exports in packages/bindings/index.js. Manifest
# napi_surface_digest must match assemble-time validation, so the post-build
# surface must already be committed (HEAD). Run after build-native-addon.sh
# and before write-native-artifact-manifest.sh — not inside the fingerprinted
# compile recipe.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

SURFACE=(
  packages/bindings/index.js
  packages/bindings/index.d.ts
)

if ! git diff --exit-code -- "${SURFACE[@]}" >/dev/null 2>&1; then
  echo "assert-bindings-napi-surface-clean: napi build changed committed surface files:" >&2
  git diff -- "${SURFACE[@]}" >&2 || true
  echo >&2
  echo "Regenerate from packages/bindings and commit the surface:" >&2
  echo "  npx napi build --platform --release --features otel --target <triple>" >&2
  echo "Then commit packages/bindings/index.js and index.d.ts if changed." >&2
  exit 1
fi

echo "ok: bindings napi surface matches HEAD"
