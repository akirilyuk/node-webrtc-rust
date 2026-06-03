#!/usr/bin/env bash
# Regenerate package-lock optional @node-webrtc-rust/bindings-* platform entries.
#
# Release prep sometimes leaves stub lock entries (`"optional": true` only, no
# version/resolved/integrity). npm ci then fails with "Invalid Version:".
#
# Usage: bash scripts/ci/refresh-package-lock-optional-bindings.sh
#
# Safe to run after bump-workspace-versions.sh when platform packages exist on
# npm at the bumped version. Before first publish, npm install may 404 — stubs
# are still pruned so a follow-up run after publish fixes CI.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LOCK="$ROOT/package-lock.json"

if [[ ! -f "$LOCK" ]]; then
  echo "ERROR: missing $LOCK" >&2
  exit 1
fi

echo "==> Prune incomplete optional binding entries from package-lock.json"
node -e "
const fs = require('fs');
const lockPath = process.argv[1];
const lock = JSON.parse(fs.readFileSync(lockPath, 'utf8'));
let removed = 0;
for (const key of Object.keys(lock.packages)) {
  if (!key.startsWith('packages/bindings/node_modules/@node-webrtc-rust/bindings-')) continue;
  const entry = lock.packages[key];
  if (!entry.version) {
    delete lock.packages[key];
    removed++;
  }
}
fs.writeFileSync(lockPath, JSON.stringify(lock, null, 2) + '\n');
console.log('removed ' + removed + ' stub optional binding entries');
" "$LOCK"

echo "==> npm install (refresh optional binding lock metadata)"
cd "$ROOT"
if npm install; then
  echo "==> package-lock optional bindings refreshed"
else
  echo "WARN: npm install failed — platform @node-webrtc-rust/bindings-* packages may not be on npm yet." >&2
  echo "After publish, re-run: bash scripts/ci/refresh-package-lock-optional-bindings.sh" >&2
  exit 1
fi
