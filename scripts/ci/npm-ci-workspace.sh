#!/usr/bin/env bash
# Clean npm ci for CI and Docker test runs.
# package-lock.json may only list another OS rollup optional entry (npm/cli#4828).
set -euo pipefail

echo "==> npm ci (clean)"
rm -rf node_modules packages/*/node_modules
npm ci

bash scripts/fix-rollup-native.sh
