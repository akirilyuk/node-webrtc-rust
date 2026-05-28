#!/usr/bin/env bash
# Set release versions and cross-package dependency pins without npm registry
# resolution. `npm pkg set` / `npm version --workspace` can trigger lockfile
# updates that fetch unpublished packages (e.g. @node-webrtc-rust/helpers on
# first release).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BINDINGS="$ROOT/packages/bindings"
VERSION="${1:-}"

if [[ -z "$VERSION" ]]; then
  echo "Usage: $0 <version>" >&2
  exit 1
fi

set_json_field() {
  local file="$1" field="$2" value="$3"
  node -e "
    const fs = require('fs');
    const p = JSON.parse(fs.readFileSync('$file', 'utf8'));
    const keys = '$field'.split('.');
    let obj = p;
    for (let i = 0; i < keys.length - 1; i++) {
      if (!obj[keys[i]]) obj[keys[i]] = {};
      obj = obj[keys[i]];
    }
    obj[keys[keys.length - 1]] = '$value';
    fs.writeFileSync('$file', JSON.stringify(p, null, 2) + '\n');
  "
}

echo "==> Set release dependency versions to $VERSION"

for opt in \
  bindings-darwin-arm64 \
  bindings-darwin-x64 \
  bindings-linux-x64-gnu \
  bindings-linux-x64-musl \
  bindings-linux-arm64-gnu \
  bindings-win32-x64-msvc; do
  set_json_field "$BINDINGS/package.json" "optionalDependencies.@node-webrtc-rust/${opt}" "$VERSION"
done

for pkg in sdk signaling helpers; do
  set_json_field "$ROOT/packages/$pkg/package.json" "version" "$VERSION"
done

set_json_field "$ROOT/packages/sdk/package.json" "dependencies.@node-webrtc-rust/bindings" "$VERSION"
set_json_field "$ROOT/packages/sdk/package.json" "dependencies.@node-webrtc-rust/signaling" "$VERSION"
set_json_field "$ROOT/packages/helpers/package.json" "dependencies.@node-webrtc-rust/sdk" "$VERSION"
set_json_field "$ROOT/packages/helpers/package.json" "dependencies.@node-webrtc-rust/signaling" "$VERSION"
