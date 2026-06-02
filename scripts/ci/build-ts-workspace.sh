#!/usr/bin/env bash
# Build sdk + signaling despite the sdk↔signaling import cycle (conference/ imports signaling).
#
# Order:
#   1. sdk core (without conference/) so signaling can resolve sdk types
#   2. signaling (imports sdk dist)
#   3. full sdk rebuild (conference/ + signaling-bridge; signaling dist now exists)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "==> clean sdk dist"
rm -rf packages/sdk/dist

echo "==> build sdk core cjs (without conference/)"
npx tsc -p scripts/ci/tsconfig.build-sdk.cjs.json

echo "==> build sdk core esm"
npx tsc -p scripts/ci/tsconfig.build-sdk.esm.json
node -e "require('fs').writeFileSync('packages/sdk/dist/esm/package.json', '{\"type\":\"module\"}\n')"

echo "==> build signaling"
npm run build --workspace=@node-webrtc-rust/signaling

echo "==> build full sdk (includes conference/)"
npm run build --workspace=@node-webrtc-rust/sdk

echo "==> build helpers"
npm run build --workspace=@node-webrtc-rust/helpers

KEY="$(bash "$ROOT/scripts/ci/ts-dist-cache-key.sh")"
mkdir -p packages/sdk/dist
echo "$KEY" > packages/sdk/dist/.ci-ts-dist-key
echo "    stamped dist fingerprint ${KEY:0:12}…"

echo "==> TypeScript workspace build OK"
