#!/usr/bin/env bash
# Fail fast when package-lock.json has stub optional @node-webrtc-rust/bindings-* entries.
#
# Release prep can leave `"optional": true` only (no version/resolved/integrity).
# npm ci then fails with an opaque "Invalid Version:" error.
#
# Usage: bash scripts/ci/validate-package-lock-optional-bindings.sh
#        npm run ci:validate:package-lock
#
# CI job: validate-package-lock (always runs on PR / main / release — no path filter)
# Docs: scripts/RELEASE.md#package-lockjson-after-release
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LOCK="$ROOT/package-lock.json"

if [[ ! -f "$LOCK" ]]; then
  echo "ERROR: missing $LOCK" >&2
  exit 1
fi

node -e "
const fs = require('fs');
const lockPath = process.argv[1];
const lock = JSON.parse(fs.readFileSync(lockPath, 'utf8'));
const stubs = [];
for (const key of Object.keys(lock.packages)) {
  if (!key.startsWith('packages/bindings/node_modules/@node-webrtc-rust/bindings-')) continue;
  if (!lock.packages[key].version) stubs.push(key.replace('packages/bindings/node_modules/', ''));
}
if (stubs.length === 0) process.exit(0);
console.error(
  'package-lock.json has ' +
    stubs.length +
    ' stub optional binding entries (missing version). npm ci fails with \"Invalid Version:\".'
);
console.error('Stubs: ' + stubs.join(', '));
console.error('');
console.error('Fix (platform packages must exist on npm at the workspace version):');
console.error('  bash scripts/ci/refresh-package-lock-optional-bindings.sh');
console.error('Then commit package-lock.json.');
process.exit(1);
" "$LOCK"
