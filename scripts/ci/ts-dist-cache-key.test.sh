#!/usr/bin/env bash
# Contract tests for TypeScript dist cache key inputs.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

KEY="$ROOT/scripts/ci/ts-dist-cache-key.sh"
chmod +x "$KEY"

export TS_DIST_NODE_MAJOR=20
base="$(bash "$KEY")"
base2="$(bash "$KEY")"
if [[ "$base" != "$base2" || ${#base} -ne 64 ]]; then
  echo "FAIL: unstable ts-dist key" >&2
  exit 1
fi
echo "ok: stable 64-char key"

export TS_DIST_NODE_MAJOR=22
node22="$(bash "$KEY")"
if [[ "$node22" == "$base" ]]; then
  echo "FAIL: Node major must change key" >&2
  exit 1
fi
echo "ok: Node major discriminator"
export TS_DIST_NODE_MAJOR=20

# Action must not namespace by pr/release (identical build contract).
action=".github/actions/ci-cache-ts-dist/action.yml"
if grep -qE 'ts-dist-\$\{\{\s*inputs\.profile' "$action"; then
  echo "FAIL: action still namespaces by profile" >&2
  exit 1
fi
if ! grep -q 'ts-dist-v2-node' "$action"; then
  echo "FAIL: action missing ts-dist-v2-node key prefix" >&2
  exit 1
fi
echo "ok: action key shape (no pr/release namespace)"

# hashFiles / script must cover manifests + build scripts.
for needle in \
  package-lock.json \
  packages/sdk/package.json \
  scripts/ci/build-ts-workspace.sh \
  packages/sdk/src; do
  if ! grep -q "$needle" "$KEY"; then
    echo "FAIL: ts-dist-cache-key.sh missing input path mention: $needle" >&2
    exit 1
  fi
done
echo "ok: key script lists required inputs"

# ensure-ts-dist rebuilds when stamp mismatches
ensure="$ROOT/scripts/ci/ensure-ts-dist.sh"
if ! grep -q 'STAMP_FILE' "$ensure" || ! grep -q 'TS_DIST_CACHE_HIT' "$ensure"; then
  echo "FAIL: ensure-ts-dist missing stamp/cache-hit validation" >&2
  exit 1
fi
echo "ok: ensure-ts-dist validates stamp"

echo "ts-dist-cache-key.test.sh: all checks passed"
