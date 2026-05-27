#!/usr/bin/env bash
# Rollup lists platform binaries as optionalDependencies; npm may skip the host
# entry when package-lock was generated on another OS (npm/cli#4828).
set -euo pipefail

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
