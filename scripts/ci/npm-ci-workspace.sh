#!/usr/bin/env bash
# Clean npm ci for CI and Docker test runs.
# package-lock.json may only list another OS rollup optional entry (npm/cli#4828).
set -euo pipefail

bash scripts/ci/validate-package-lock-optional-bindings.sh

echo "==> npm ci (clean)"
rm -rf node_modules packages/*/node_modules
npm ci

bash scripts/fix-rollup-native.sh
bash scripts/ci/ensure-workspace-bindings.sh
