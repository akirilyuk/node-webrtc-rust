#!/usr/bin/env bash
# Vitest roundtrip evaluators load @node-webrtc-rust/bindings, which may pull a
# platform optional package. On release prep PRs, optionalDependencies can be
# bumped before that version is published — install the latest published build instead.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BINDINGS="$ROOT/packages/bindings"

shopt -s nullglob
local_nodes=("$BINDINGS"/*.node)
if [[ ${#local_nodes[@]} -gt 0 ]]; then
  echo "==> vitest bindings: using workspace ${local_nodes[0]##*/}"
  exit 0
fi

detect_optional_pkg() {
  local os arch
  os=$(uname -s | tr '[:upper:]' '[:lower:]')
  arch=$(uname -m)
  case "$os" in
    darwin)
      case "$arch" in
        arm64 | aarch64) echo "@node-webrtc-rust/bindings-darwin-arm64" ;;
        x86_64) echo "@node-webrtc-rust/bindings-darwin-x64" ;;
        *) echo "unsupported macOS arch: $arch" >&2; return 1 ;;
      esac
      ;;
    linux)
      local libc="gnu"
      if ldd --version 2>&1 | grep -qi musl || ([[ -f /etc/os-release ]] && grep -qi alpine /etc/os-release); then
        libc="musl"
      fi
      case "$arch" in
        x86_64)
          if [[ "$libc" == "musl" ]]; then
            echo "@node-webrtc-rust/bindings-linux-x64-musl"
          else
            echo "@node-webrtc-rust/bindings-linux-x64-gnu"
          fi
          ;;
        aarch64 | arm64) echo "@node-webrtc-rust/bindings-linux-arm64-gnu" ;;
        *) echo "unsupported Linux arch: $arch" >&2; return 1 ;;
      esac
      ;;
    mingw* | msys* | cygwin*) echo "@node-webrtc-rust/bindings-win32-x64-msvc" ;;
    *) echo "unsupported OS for vitest bindings: $os" >&2; return 1 ;;
  esac
}

PKG="$(detect_optional_pkg)"
WANT="$(node -p "require('$BINDINGS/package.json').optionalDependencies['$PKG'] || ''")"
if [[ -z "$WANT" ]]; then
  echo "ensure-vitest-optional-bindings: no optionalDependency for $PKG" >&2
  exit 1
fi

if npm view "${PKG}@${WANT}" version &>/dev/null; then
  echo "==> vitest bindings: ${PKG}@${WANT} on npm"
  (cd "$BINDINGS" && npm install --no-save "${PKG}@${WANT}")
  exit 0
fi

PUBLISHED="$(npm view "$PKG" version)"
echo "==> vitest bindings: ${PKG}@${WANT} not on npm yet; using published @${PUBLISHED}"
(cd "$BINDINGS" && npm install --no-save "${PKG}@${PUBLISHED}")
