#!/usr/bin/env bash
# Contract tests for assert-bindings-napi-surface-clean.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

SCRIPT="$ROOT/scripts/ci/assert-bindings-napi-surface-clean.sh"
INDEX="$ROOT/packages/bindings/index.js"
LINUX=".github/actions/ci-build-native-linux/action.yml"
HOST=".github/actions/ci-build-native-host/action.yml"

chmod +x "$SCRIPT"

echo "==> pass when surface matches HEAD"
bash "$SCRIPT"

echo "==> fail when index.js drifts from HEAD"
backup="$(mktemp)"
cp "$INDEX" "$backup"
trap 'mv "$backup" "$INDEX"' EXIT
printf '\n// drift probe\n' >>"$INDEX"
if bash "$SCRIPT" 2>/tmp/assert-surface.err; then
  echo "FAIL: assert accepted drifted index.js" >&2
  exit 1
fi
rg -q 'assert-bindings-napi-surface-clean' /tmp/assert-surface.err
rg -q 'napi build' /tmp/assert-surface.err
mv "$backup" "$INDEX"
trap - EXIT
echo "ok: drift rejected with actionable message"

echo "==> wired in linux/host actions between build and manifest"
for action in "$LINUX" "$HOST"; do
  grep -q 'assert-bindings-napi-surface-clean.sh' "$action" || {
    echo "FAIL: missing assert step in $action" >&2
    exit 1
  }
done
python3 - "$LINUX" "$HOST" <<'PY'
from pathlib import Path
import re
import sys

for path in sys.argv[1:]:
    text = Path(path).read_text(encoding="utf-8")
    build = text.find("build-native-addon.sh")
    assert_step = text.find("assert-bindings-napi-surface-clean.sh")
    manifest = text.find("write-native-artifact-manifest.sh")
    if build < 0 or assert_step < 0 or manifest < 0:
        raise SystemExit(f"{path}: missing build, assert, or manifest step")
    if not (build < assert_step < manifest):
        raise SystemExit(
            f"{path}: assert-bindings-napi-surface-clean must follow build "
            "and precede write-native-artifact-manifest"
        )
print("ok: action step order")
PY

echo "assert-bindings-napi-surface-clean.test.sh: all checks passed"
