#!/usr/bin/env bash
# Copy workspace packages/bindings into nested registry installs under packages/sdk.
#
# npm ci hoists workspace bindings at repo root, but packages/sdk still resolves
# @node-webrtc-rust/bindings@0.1.5 from the registry (no JsVoiceAgent / ICE APIs).
# CI compiles a fresh .node artifact; locally we must sync index + .node after build:native.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SRC="$ROOT/packages/bindings"

if [[ ! -f "$SRC/index.js" ]]; then
  echo "sync-workspace-bindings: missing $SRC/index.js — run npm run build:native first" >&2
  exit 1
fi

shopt -s nullglob
nodes=("$SRC"/*.node)

sync_into() {
  local dst="$1"
  if [[ ! -d "$dst" ]]; then
    return 0
  fi
  echo "==> sync workspace bindings → ${dst#$ROOT/}"
  cp "$SRC/index.js" "$SRC/index.d.ts" "$dst/"
  for node in "${nodes[@]}"; do
    cp "$node" "$dst/"
  done
}

sync_into "$ROOT/packages/sdk/node_modules/@node-webrtc-rust/bindings"

# signaling depends on sdk; nested sdk may pull its own bindings copy
sync_into "$ROOT/packages/signaling/node_modules/@node-webrtc-rust/sdk/node_modules/@node-webrtc-rust/bindings"

echo "==> workspace bindings synced for npm test"
