#!/usr/bin/env bash
# Remove nested registry @node-webrtc-rust/bindings copies under packages/* and
# examples/* so npm test loads packages/bindings/*.node (workspace build).
#
# npm ci can install a registry stub under packages/sdk/node_modules/.../bindings
# (no local .node). Node resolves that path first and falls back to published
# optional platform packages — stale vs workspace NAPI surface.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

shopt -s nullglob
for nested in \
  packages/*/node_modules/@node-webrtc-rust/bindings \
  examples/*/node_modules/@node-webrtc-rust/bindings; do
  if [[ -d "$nested" && ! -L "$nested" ]]; then
    echo "==> remove nested registry bindings: ${nested#"$ROOT"/}"
    rm -rf "$nested"
  fi
done
