#!/usr/bin/env bash
# Bump all @node-webrtc-rust/* package.json versions and internal pins.
# Avoids npm registry resolution (unlike `npm version --workspace`).
#
# Usage: bash scripts/ci/bump-workspace-versions.sh <version>
#
# Used by release-prep/* PRs, version catch-up PRs, release-local.sh, and
# post-release-sync-main-package-lock.sh (after npm publish).
#
# Release prep (before platform packages are on npm):
#   SKIP_LOCK_REFRESH=1 bash scripts/ci/bump-workspace-versions.sh <version>
# Lock refresh runs automatically in CI after publish (see RELEASE.md).
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

bump_node_webrtc_deps() {
  local file="$1"
  node -e "
    const fs = require('fs');
    const f = '$file';
    const version = '$VERSION';
    const p = JSON.parse(fs.readFileSync(f, 'utf8'));
    let changed = false;
    for (const depType of ['dependencies', 'devDependencies', 'peerDependencies', 'optionalDependencies']) {
      if (!p[depType]) continue;
      for (const k of Object.keys(p[depType])) {
        if (!k.startsWith('@node-webrtc-rust/')) continue;
        const cur = p[depType][k];
        if (typeof cur === 'string' && cur.startsWith('^')) {
          p[depType][k] = '^' + version;
        } else {
          p[depType][k] = version;
        }
        changed = true;
      }
    }
    if (changed) fs.writeFileSync(f, JSON.stringify(p, null, 2) + '\n');
  "
}

echo "==> Bumping workspace package versions to $VERSION"

set_json_field "$BINDINGS/package.json" "version" "$VERSION"
set_json_field "$ROOT/packages/sdk/package.json" "version" "$VERSION"
set_json_field "$ROOT/packages/signaling/package.json" "version" "$VERSION"
set_json_field "$ROOT/packages/helpers/package.json" "version" "$VERSION"

set_json_field "$ROOT/packages/sdk/package.json" "dependencies.@node-webrtc-rust/bindings" "$VERSION"
set_json_field "$ROOT/packages/sdk/package.json" "dependencies.@node-webrtc-rust/signaling" "$VERSION"
set_json_field "$ROOT/packages/helpers/package.json" "dependencies.@node-webrtc-rust/sdk" "$VERSION"
set_json_field "$ROOT/packages/helpers/package.json" "dependencies.@node-webrtc-rust/signaling" "$VERSION"

for opt in \
  bindings-darwin-arm64 \
  bindings-darwin-x64 \
  bindings-linux-x64-gnu \
  bindings-linux-x64-musl \
  bindings-linux-arm64-gnu \
  bindings-win32-x64-msvc; do
  set_json_field "$BINDINGS/package.json" "optionalDependencies.@node-webrtc-rust/${opt}" "$VERSION"
done

for dir in "$BINDINGS"/npm/*/; do
  if [[ -f "${dir}package.json" ]]; then
    set_json_field "${dir}package.json" "version" "$VERSION"
  fi
done

for pkg in sdk signaling helpers; do
  bump_node_webrtc_deps "$ROOT/packages/$pkg/package.json"
done

for dir in "$ROOT"/examples/*/; do
  if [[ -f "${dir}package.json" ]]; then
    set_json_field "${dir}package.json" "version" "$VERSION"
    bump_node_webrtc_deps "${dir}package.json"
  fi
done

if [[ "${SKIP_LOCK_REFRESH:-0}" == "1" ]]; then
  echo "==> SKIP_LOCK_REFRESH=1 — skipping package-lock refresh (run after npm publish or merge the post-release PR)" >&2
else
  echo "==> Refreshing package-lock optional binding entries"
  if ! bash "$ROOT/scripts/ci/refresh-package-lock-optional-bindings.sh"; then
    echo "ERROR: lockfile refresh failed (platform bindings @$VERSION may not be on npm yet)." >&2
    echo "Before publish: SKIP_LOCK_REFRESH=1 bash scripts/ci/bump-workspace-versions.sh $VERSION" >&2
    echo "After publish: bash scripts/ci/post-release-sync-main-package-lock.sh $VERSION (or merge the automated PR)" >&2
    exit 1
  fi

  echo "==> Validate package-lock optional bindings"
  bash "$ROOT/scripts/ci/validate-package-lock-optional-bindings.sh"
fi

echo "==> Done — all publishable packages and examples set to $VERSION"
