#!/usr/bin/env bash
# Clean npm ci for CI and Docker test runs.
# package-lock.json may only list another OS rollup optional entry (npm/cli#4828).
set -euo pipefail

echo "==> npm ci (clean)"
rm -rf node_modules packages/*/node_modules
npm ci

rollup_pkg=""
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) rollup_pkg="@rollup/rollup-linux-x64-gnu" ;;
  Linux-aarch64) rollup_pkg="@rollup/rollup-linux-arm64-gnu" ;;
  Darwin-arm64) rollup_pkg="@rollup/rollup-darwin-arm64" ;;
  Darwin-x86_64) rollup_pkg="@rollup/rollup-darwin-x64" ;;
esac

if [[ -n "$rollup_pkg" && ! -d "node_modules/${rollup_pkg}" ]]; then
  rollup_ver="$(node -p "require('rollup/package.json').optionalDependencies['${rollup_pkg}'] || ''")"
  if [[ -n "$rollup_ver" ]]; then
    echo "==> install ${rollup_pkg}@${rollup_ver} (rollup optional native binary)"
    npm install --no-save "${rollup_pkg}@${rollup_ver}"
  fi
fi
