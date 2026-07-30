#!/usr/bin/env bash
# Compatibility tests for native-binding-cache-key.sh (wraps native-build-fingerprint).
# Full contract coverage lives in native-build-fingerprint.test.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

KEY="$ROOT/scripts/ci/native-binding-cache-key.sh"
FP="$ROOT/scripts/ci/native-build-fingerprint.sh"
PKG="$ROOT/packages/bindings/package.json"

export NATIVE_TOOL_MODE=declared
chmod +x "$KEY" "$FP" scripts/ci/collect-native-tool-identity.sh

pkg_backup=""
cleanup() {
  if [[ -n "$pkg_backup" && -f "$pkg_backup" ]]; then
    cp "$pkg_backup" "$PKG"
    rm -f "$pkg_backup"
  fi
}
trap cleanup EXIT

TARGET=x86_64-unknown-linux-gnu

base_hash="$(bash "$KEY" --target "$TARGET" --profile release)"
direct="$(bash "$FP" --target "$TARGET" --profile release)"
if [[ "$base_hash" != "$direct" ]]; then
  echo "FAIL: cache-key wrapper != fingerprint ($base_hash vs $direct)" >&2
  exit 1
fi
echo "ok: wrapper matches fingerprint for --target"

pkg_backup="$(mktemp)"
cp "$PKG" "$pkg_backup"
python3 - <<'PY'
import json
path = "packages/bindings/package.json"
with open(path, encoding="utf-8") as f:
    data = json.load(f)
data["version"] = "99.99.99-test-only"
with open(path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
version_hash="$(bash "$KEY" --target "$TARGET" --profile release)"
if [[ "$version_hash" != "$base_hash" ]]; then
  echo "FAIL: version-only bump changed native key ($base_hash -> $version_hash)" >&2
  exit 1
fi
echo "ok: version-only package.json change preserves native key"

# Missing --target must fail (no silent aggregate).
if bash "$KEY" >/dev/null 2>&1; then
  echo "FAIL: cache-key without --target should exit non-zero" >&2
  exit 1
fi
echo "ok: --target required"

agg1="$(bash "$KEY" --aggregate --profile release)"
agg2="$(bash "$KEY" --aggregate --profile release)"
if [[ "$agg1" != "$agg2" || ${#agg1} -ne 64 ]]; then
  echo "FAIL: --aggregate digest unstable" >&2
  exit 1
fi
echo "ok: --aggregate digest"

echo "native-binding-cache-key.test.sh: all checks passed"
