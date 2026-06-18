#!/usr/bin/env bash
# Sync package-lock.json workspace package versions from package.json.
#
# Release prep bumps package.json before platform packages exist on npm at the new
# version. npm ci then fails with "Missing: @node-webrtc-rust/bindings-*@".
# This updates lock workspace versions (and optional binding entry versions) while
# keeping existing resolved/integrity URLs until refresh-package-lock-optional-bindings.sh
# runs after publish.
#
# Usage: bash scripts/ci/sync-lock-workspace-versions.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LOCK="$ROOT/package-lock.json"

node -e "
const fs = require('fs');
const path = require('path');

const root = process.argv[1];
const lockPath = path.join(root, 'package-lock.json');
const lock = JSON.parse(fs.readFileSync(lockPath, 'utf8'));

function syncLockEntry(rel) {
  const pkgPath = path.join(root, rel, 'package.json');
  if (!fs.existsSync(pkgPath)) return;
  const pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf8'));
  if (!lock.packages[rel]) return;
  lock.packages[rel].version = pkg.version;
  for (const field of [
    'dependencies',
    'devDependencies',
    'peerDependencies',
    'optionalDependencies',
  ]) {
    if (pkg[field]) lock.packages[rel][field] = pkg[field];
  }
}

for (const rel of [
  'packages/bindings',
  'packages/helpers',
  'packages/sdk',
  'packages/signaling',
]) {
  syncLockEntry(rel);
}

for (const name of fs.readdirSync(path.join(root, 'examples'))) {
  syncLockEntry('examples/' + name);
}

const bindingsVersion = JSON.parse(
  fs.readFileSync(path.join(root, 'packages/bindings/package.json'), 'utf8')
).version;

for (const key of Object.keys(lock.packages)) {
  if (!key.includes('@node-webrtc-rust/bindings-')) continue;
  if (!lock.packages[key].version) continue;
  lock.packages[key].version = bindingsVersion;
}

fs.writeFileSync(lockPath, JSON.stringify(lock, null, 2) + '\n');
console.log('Synced workspace versions in package-lock.json to match package.json');
" "$ROOT"

bash "$ROOT/scripts/ci/validate-package-lock-optional-bindings.sh"
