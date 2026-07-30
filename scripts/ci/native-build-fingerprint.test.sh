#!/usr/bin/env bash
# Deterministic tests for native-build-fingerprint / native_build_contract.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

FP="$ROOT/scripts/ci/native-build-fingerprint.sh"
KEY="$ROOT/scripts/ci/native-binding-cache-key.sh"
PY="$ROOT/scripts/ci/native_build_contract.py"
EPOCH="$ROOT/scripts/ci/native-cache-epoch"
MUSL_SCRIPT="$ROOT/scripts/ci/verify-musl-runtime.sh"
PKG="$ROOT/packages/bindings/package.json"
DTS="$ROOT/packages/bindings/index.d.ts"
CARGO_WS="$ROOT/Cargo.toml"

# Match CI / native-binding-cache-key.sh (forces declared). Without this, local
# fingerprint defaults to unresolved tool slots and the wrapper comparison flakes.
export NATIVE_TOOL_MODE=declared

chmod +x "$FP" "$KEY" scripts/ci/native-artifact-manifest.sh scripts/ci/check-release-targets.sh \
  scripts/ci/list-release-targets.sh scripts/ci/collect-native-tool-identity.sh

GNU=x86_64-unknown-linux-gnu
MUSL=x86_64-unknown-linux-musl

pkg_backup=""
dts_backup=""
epoch_backup=""
musl_backup=""
cargo_backup=""

cleanup() {
  if [[ -n "$pkg_backup" && -f "$pkg_backup" ]]; then
    cp "$pkg_backup" "$PKG"
    rm -f "$pkg_backup"
  fi
  if [[ -n "$dts_backup" && -f "$dts_backup" ]]; then
    cp "$dts_backup" "$DTS"
    rm -f "$dts_backup"
  fi
  if [[ -n "$epoch_backup" && -f "$epoch_backup" ]]; then
    cp "$epoch_backup" "$EPOCH"
    rm -f "$epoch_backup"
  fi
  if [[ -n "$musl_backup" && -f "$musl_backup" ]]; then
    cp "$musl_backup" "$MUSL_SCRIPT"
    rm -f "$musl_backup"
  fi
  if [[ -n "$cargo_backup" && -f "$cargo_backup" ]]; then
    cp "$cargo_backup" "$CARGO_WS"
    rm -f "$cargo_backup"
  fi
}
trap cleanup EXIT

digest() {
  bash "$FP" --target "$1" --profile "${2:-release}"
}

dist_digest() {
  bash "$FP" --distribution --target "$1"
}

echo "==> invariance: same inputs → same digest"
d1="$(digest "$GNU")"
d2="$(digest "$GNU")"
if [[ "$d1" != "$d2" ]]; then
  echo "FAIL: fingerprint not deterministic ($d1 vs $d2)" >&2
  exit 1
fi
echo "ok: deterministic fingerprint"

echo "==> profile features invalidate"
debug_d="$(digest "$GNU" debug)"
if [[ "$debug_d" == "$d1" ]]; then
  echo "FAIL: debug profile digest equals release" >&2
  exit 1
fi
echo "ok: debug vs release digests differ"

echo "==> musl isolation: musl-only file must not invalidate gnu"
musl_base="$(digest "$MUSL")"
gnu_base="$(digest "$GNU")"
musl_backup="$(mktemp)"
cp "$MUSL_SCRIPT" "$musl_backup"
echo "# native-build-fingerprint.test.sh marker $(date +%s)" >>"$MUSL_SCRIPT"
musl_after="$(digest "$MUSL")"
gnu_after="$(digest "$GNU")"
if [[ "$musl_after" == "$musl_base" ]]; then
  echo "FAIL: musl digest unchanged after musl-only script edit" >&2
  exit 1
fi
if [[ "$gnu_after" != "$gnu_base" ]]; then
  echo "FAIL: gnu digest changed after musl-only script edit" >&2
  exit 1
fi
cp "$musl_backup" "$MUSL_SCRIPT"
rm -f "$musl_backup"
musl_backup=""
echo "ok: musl-only invalidation is target-specific"

echo "==> complete Cargo dependency discovery (no signaling; includes sherpa)"
crate_list="$(bash "$FP" --list-local-crates --target "$GNU" --profile release)"
echo "$crate_list" | rg -q 'node-webrtc-rust-bindings@'
echo "$crate_list" | rg -q 'node-webrtc-rust-vendor-sherpa-onnx@'
echo "$crate_list" | rg -q 'node-webrtc-rust-speech@'
if echo "$crate_list" | rg -q 'node-webrtc-rust-signaling@'; then
  echo "FAIL: signaling appeared in bindings local closure" >&2
  exit 1
fi
# Ensure discovery uses cargo metadata, not a single-manifest grep (regression).
if ! python3 - <<'PY'
import importlib.util
from pathlib import Path

root = Path(".").resolve()
spec = importlib.util.spec_from_file_location(
    "native_build_contract", root / "scripts/ci/native_build_contract.py"
)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
meta = {
    "packages": [
        {
            "id": "bindings",
            "name": "node-webrtc-rust-bindings",
            "version": "0.1.0",
            "source": None,
            "manifest_path": str(root / "packages/bindings/Cargo.toml"),
        },
        {
            "id": "transitive",
            "name": "node-webrtc-rust-fake-transitive",
            "version": "0.1.0",
            "source": None,
            "manifest_path": str(root / "crates/core/Cargo.toml"),
        },
        {
            "id": "registry",
            "name": "serde",
            "version": "1.0.0",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "manifest_path": "/tmp/serde/Cargo.toml",
        },
    ],
    "resolve": {
        "nodes": [
            {"id": "bindings", "deps": [{"name": "fake", "pkg": "transitive"}], "features": ["otel"]},
            {"id": "transitive", "deps": [], "features": []},
            {"id": "registry", "deps": [], "features": []},
        ]
    },
}
# Point transitive crate_root at an existing local tree for hashing helpers.
closure = mod.local_dependency_closure(meta)
names = {c["name"] for c in closure}
assert "node-webrtc-rust-bindings" in names
assert "node-webrtc-rust-fake-transitive" in names
assert "serde" not in names
print("ok: synthetic transitive local crate discovered via resolve walk")
PY
then
  echo "FAIL: synthetic transitive discovery" >&2
  exit 1
fi
echo "ok: complete Cargo local dependency discovery"

echo "==> npm package.json version does NOT change native digest; Cargo version does"
pkg_backup="$(mktemp)"
cp "$PKG" "$pkg_backup"
base_native="$(digest "$GNU")"
base_dist="$(dist_digest "$GNU")"
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
npm_ver_native="$(digest "$GNU")"
npm_ver_dist="$(dist_digest "$GNU")"
if [[ "$npm_ver_native" != "$base_native" ]]; then
  echo "FAIL: npm version-only bump changed native digest" >&2
  exit 1
fi
if [[ "$npm_ver_dist" == "$base_dist" ]]; then
  echo "FAIL: npm version-only bump did not change distribution digest" >&2
  exit 1
fi
cp "$pkg_backup" "$PKG"
echo "ok: npm version excluded from native digest, included in distribution"

cargo_backup="$(mktemp)"
cp "$CARGO_WS" "$cargo_backup"
python3 - <<'PY'
from pathlib import Path
path = Path("Cargo.toml")
text = path.read_text(encoding="utf-8")
old = 'version = "0.1.0"'
new = 'version = "0.1.0-fingerprint-test"'
# Only rewrite the workspace.package version block's first version line.
idx = text.find("[workspace.package]")
if idx < 0:
    raise SystemExit("missing [workspace.package]")
head, rest = text[:idx], text[idx:]
rest2 = rest.replace(old, new, 1)
if rest2 == rest:
    raise SystemExit("workspace version line not found")
path.write_text(head + rest2, encoding="utf-8")
PY
cargo_ver_native="$(digest "$GNU")"
if [[ "$cargo_ver_native" == "$base_native" ]]; then
  echo "FAIL: Cargo workspace version bump did not change native digest" >&2
  exit 1
fi
cp "$cargo_backup" "$CARGO_WS"
rm -f "$cargo_backup"
cargo_backup=""
echo "ok: Cargo version invalidates native digest"

echo "==> distribution surface (index.d.ts) does not change native digest"
dts_backup="$(mktemp)"
cp "$DTS" "$dts_backup"
dist_before="$(dist_digest "$GNU")"
native_before="$(digest "$GNU")"
echo "// native-build-fingerprint.test.sh marker" >>"$DTS"
dist_after="$(dist_digest "$GNU")"
native_after="$(digest "$GNU")"
if [[ "$native_after" != "$native_before" ]]; then
  echo "FAIL: index.d.ts change invalidated native digest" >&2
  exit 1
fi
if [[ "$dist_after" == "$dist_before" ]]; then
  echo "FAIL: index.d.ts change did not invalidate distribution digest" >&2
  exit 1
fi
cp "$dts_backup" "$DTS"
rm -f "$dts_backup"
dts_backup=""
echo "ok: distribution digest tracks generated surface; native digest does not"

echo "==> cache epoch invalidates"
epoch_backup="$(mktemp)"
cp "$EPOCH" "$epoch_backup"
before="$(digest "$GNU")"
echo "fingerprint-test-epoch" >"$EPOCH"
after="$(digest "$GNU")"
if [[ "$after" == "$before" ]]; then
  echo "FAIL: cache epoch change did not invalidate digest" >&2
  exit 1
fi
cp "$epoch_backup" "$EPOCH"
rm -f "$epoch_backup"
epoch_backup=""
echo "ok: cache epoch invalidates"

echo "==> wrapper --target matches fingerprint script"
wrap="$(NATIVE_TARGET="$GNU" NATIVE_PROFILE=release bash "$KEY")"
direct="$(digest "$GNU")"
if [[ "$wrap" != "$direct" ]]; then
  echo "FAIL: native-binding-cache-key --target mismatch" >&2
  exit 1
fi
echo "ok: cache-key wrapper matches fingerprint"

echo "==> check-release-targets completeness"
python3 "$PY" check-release-targets
echo "ok: six-target completeness"

echo "native-build-fingerprint.test.sh: all checks passed"
