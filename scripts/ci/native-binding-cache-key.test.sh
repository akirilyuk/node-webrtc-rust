#!/usr/bin/env bash
# Deterministic tests for native-binding-cache-key.sh (no GitHub Actions required).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

KEY="$ROOT/scripts/ci/native-binding-cache-key.sh"
PKG="$ROOT/packages/bindings/package.json"
DTS="$ROOT/packages/bindings/index.d.ts"

chmod +x "$KEY"

hash_key() {
  bash "$KEY"
}

pkg_backup=""
dts_backup=""

cleanup() {
  if [[ -n "$pkg_backup" && -f "$pkg_backup" ]]; then
    cp "$pkg_backup" "$PKG"
    rm -f "$pkg_backup"
  fi
  if [[ -n "$dts_backup" && -f "$dts_backup" ]]; then
    cp "$dts_backup" "$DTS"
    rm -f "$dts_backup"
  fi
}
trap cleanup EXIT

base_hash="$(hash_key)"
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

version_hash="$(hash_key)"
if [[ "$version_hash" != "$base_hash" ]]; then
  echo "FAIL: version-only bump changed hash ($base_hash -> $version_hash)" >&2
  exit 1
fi
echo "ok: version-only package.json change preserves hash"

cp "$pkg_backup" "$PKG"
python3 - <<'PY'
import json

path = "packages/bindings/package.json"
with open(path, encoding="utf-8") as f:
    data = json.load(f)
data.setdefault("napi", {})["name"] = "node-webrtc-rust-cache-key-test"
with open(path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY

build_hash="$(hash_key)"
if [[ "$build_hash" == "$base_hash" ]]; then
  echo "FAIL: build-relevant package.json change did not change hash" >&2
  exit 1
fi
echo "ok: build-relevant package.json change invalidates hash"

cp "$pkg_backup" "$PKG"
dts_backup="$(mktemp)"
cp "$DTS" "$dts_backup"
echo "// native-binding-cache-key.test.sh marker" >>"$DTS"

dts_hash="$(hash_key)"
if [[ "$dts_hash" == "$base_hash" ]]; then
  echo "FAIL: index.d.ts change did not change hash" >&2
  exit 1
fi
echo "ok: index.d.ts change invalidates hash"

echo "native-binding-cache-key.test.sh: all checks passed"
