#!/usr/bin/env bash
# Contract tests for bindings-napi-surface.sh (snapshot/restore).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

SCRIPT="$ROOT/scripts/ci/bindings-napi-surface.sh"
INDEX="$ROOT/packages/bindings/index.js"
WRITE_MANIFEST="$ROOT/scripts/ci/write-native-artifact-manifest.sh"
LINUX=".github/actions/ci-build-native-linux/action.yml"
HOST=".github/actions/ci-build-native-host/action.yml"
SNAP="$(mktemp -d)"
export NAPI_SURFACE_SNAPSHOT_DIR="$SNAP"
trap 'rm -rf "$SNAP"' EXIT

chmod +x "$SCRIPT"

echo "==> snapshot + restore no-op when unchanged"
bash "$SCRIPT" snapshot
out="$(bash "$SCRIPT" restore)"
rg -q 'already matches snapshot' <<<"$out"

echo "==> restore undoes drift from snapshot (no git required)"
backup="$(mktemp)"
cp "$INDEX" "$backup"
printf '\n// drift probe\n' >>"$INDEX"
bash "$SCRIPT" restore >/tmp/napi-surface-restore.out
rg -q 'restored' /tmp/napi-surface-restore.out
cmp -s "$backup" "$INDEX"
cp "$backup" "$INDEX"
rm -f "$backup"
echo "ok: snapshot restore works without git"

echo "==> restore fails closed without snapshot when ROOT has no git"
# Script resolves ROOT from its own path — copy into an isolated tree.
iso="$(mktemp -d)"
mkdir -p "$iso/packages/bindings" "$iso/scripts/ci"
cp packages/bindings/index.js packages/bindings/index.d.ts "$iso/packages/bindings/"
cp "$SCRIPT" "$iso/scripts/ci/"
(
  cd "$iso"
  unset NAPI_SURFACE_SNAPSHOT_DIR
  if bash scripts/ci/bindings-napi-surface.sh restore 2>/tmp/napi-surface-nosnap.err; then
    echo "FAIL: restore succeeded without snapshot outside git" >&2
    exit 1
  fi
  rg -q 'no snapshot' /tmp/napi-surface-nosnap.err
)
rm -rf "$iso"
export NAPI_SURFACE_SNAPSHOT_DIR="$SNAP"
echo "ok: missing snapshot fails outside git"

echo "==> write-native-artifact-manifest restores only when snapshot dir exists"
rg -q 'bindings-napi-surface.sh restore' "$WRITE_MANIFEST"
rg -q 'napi-surface-snapshot' "$WRITE_MANIFEST"


echo "==> linux/host: snapshot before build, restore after, before manifest"
for action in "$LINUX" "$HOST"; do
  grep -q 'bindings-napi-surface.sh snapshot' "$action" || {
    echo "FAIL: missing snapshot in $action" >&2
    exit 1
  }
  grep -q 'bindings-napi-surface.sh restore' "$action" || {
    echo "FAIL: missing restore in $action" >&2
    exit 1
  }
done
python3 - "$LINUX" "$HOST" <<'PY'
from pathlib import Path
import sys

for path in sys.argv[1:]:
    text = Path(path).read_text(encoding="utf-8")
    snap = text.find("bindings-napi-surface.sh snapshot")
    build = text.find("build-native-addon.sh")
    restore = text.find("bindings-napi-surface.sh restore")
    manifest = text.find("write-native-artifact-manifest.sh")
    if min(snap, build, restore, manifest) < 0:
        raise SystemExit(f"{path}: missing snapshot/build/restore/manifest")
    if not (snap < build < restore < manifest):
        raise SystemExit(
            f"{path}: expected snapshot → build → restore → manifest order"
        )
print("ok: action step order")
PY

echo "bindings-napi-surface.test.sh: all checks passed"
