#!/usr/bin/env bash
# Contract tests for restore-bindings-napi-surface-from-head.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

SCRIPT="$ROOT/scripts/ci/restore-bindings-napi-surface-from-head.sh"
INDEX="$ROOT/packages/bindings/index.js"
WRITE_MANIFEST="$ROOT/scripts/ci/write-native-artifact-manifest.sh"
LINUX=".github/actions/ci-build-native-linux/action.yml"
HOST=".github/actions/ci-build-native-host/action.yml"

chmod +x "$SCRIPT"

echo "==> no-op when surface matches HEAD"
out="$(bash "$SCRIPT")"
rg -q 'already matches HEAD' <<<"$out"

echo "==> restores drifted index.js to HEAD"
backup="$(mktemp)"
cp "$INDEX" "$backup"
trap 'cp "$backup" "$INDEX"; rm -f "$backup"' EXIT
printf '\n// drift probe\n' >>"$INDEX"
if git diff --quiet -- packages/bindings/index.js; then
  echo "FAIL: drift probe did not dirty index.js" >&2
  exit 1
fi
bash "$SCRIPT" >/tmp/restore-surface.out
rg -q 'restored' /tmp/restore-surface.out
if ! git diff --quiet -- packages/bindings/index.js; then
  echo "FAIL: index.js still dirty after restore" >&2
  exit 1
fi
cp "$backup" "$INDEX"
trap - EXIT
rm -f "$backup"
echo "ok: drift restored from HEAD"

echo "==> write-native-artifact-manifest invokes restore"
rg -q 'restore-bindings-napi-surface-from-head.sh' "$WRITE_MANIFEST" || {
  echo "FAIL: write-native-artifact-manifest must restore surface before produce" >&2
  exit 1
}

echo "==> wired in linux/host actions between build and verify/manifest"
for action in "$LINUX" "$HOST"; do
  grep -q 'restore-bindings-napi-surface-from-head.sh' "$action" || {
    echo "FAIL: missing restore step in $action" >&2
    exit 1
  }
done
python3 - "$LINUX" "$HOST" <<'PY'
from pathlib import Path
import sys

for path in sys.argv[1:]:
    text = Path(path).read_text(encoding="utf-8")
    build = text.find("build-native-addon.sh")
    restore = text.find("restore-bindings-napi-surface-from-head.sh")
    manifest = text.find("write-native-artifact-manifest.sh")
    if build < 0 or restore < 0 or manifest < 0:
        raise SystemExit(f"{path}: missing build, restore, or manifest step")
    if not (build < restore < manifest):
        raise SystemExit(
            f"{path}: restore-bindings-napi-surface-from-head must follow build "
            "and precede write-native-artifact-manifest"
        )
print("ok: action step order")
PY

echo "restore-bindings-napi-surface-from-head.test.sh: all checks passed"
