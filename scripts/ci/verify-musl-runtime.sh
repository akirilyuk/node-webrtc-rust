#!/usr/bin/env bash
# dlopen the musl .node inside node:24-alpine — catches glibc-linked Zig artifacts.
# Usage: verify-musl-runtime.sh [path/to/node-webrtc-rust.linux-x64-musl.node]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
NODE_FILE="${1:-$ROOT/packages/bindings/node-webrtc-rust.linux-x64-musl.node}"
ALPINE_NODE_IMAGE="${MUSL_RUNTIME_IMAGE:-node:24-alpine}"

if [[ ! -f "$NODE_FILE" ]]; then
  echo "Missing musl artifact: $NODE_FILE" >&2
  exit 1
fi

BINDINGS_DIR="$(cd "$(dirname "$NODE_FILE")" && pwd)"
BASENAME="$(basename "$NODE_FILE")"

echo "==> Musl runtime verify — $BASENAME"
if grep -qi alpine /etc/os-release 2>/dev/null; then
  # Already on Alpine CI (ci-build-alpine); dlopen in-process — no nested Docker.
  cd "$BINDINGS_DIR"
  node -e "
    const b = require('./${BASENAME}');
    new b.JsPeerConnection({});
    console.log('musl runtime OK (native Alpine)');
  "
else
  echo "==> Musl runtime verify ($ALPINE_NODE_IMAGE) — $BASENAME"
  docker run --rm \
    -v "${BINDINGS_DIR}:/bindings:ro" \
    -w /bindings \
    "$ALPINE_NODE_IMAGE" \
    node -e "
      const b = require('./${BASENAME}');
      new b.JsPeerConnection({});
      console.log('musl runtime OK');
    "
fi
