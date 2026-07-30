#!/usr/bin/env bash
# Tests for six-target release contract completeness.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

CHECK="$ROOT/scripts/ci/check-release-targets.sh"
LIST="$ROOT/scripts/ci/list-release-targets.sh"
chmod +x "$CHECK" "$LIST"

echo "==> list-release-targets emits exactly six canonical triples"
got="$(bash "$LIST" | tr '\n' ' ' | sed 's/[[:space:]]*$//')"
want="x86_64-unknown-linux-gnu x86_64-unknown-linux-musl aarch64-unknown-linux-gnu x86_64-apple-darwin aarch64-apple-darwin x86_64-pc-windows-msvc"
if [[ "$got" != "$want" ]]; then
  echo "FAIL: target list mismatch" >&2
  echo "  got:  $got" >&2
  echo "  want: $want" >&2
  exit 1
fi
count="$(bash "$LIST" | wc -l | tr -d ' ')"
if [[ "$count" != "6" ]]; then
  echo "FAIL: expected 6 targets, got $count" >&2
  exit 1
fi
echo "ok: canonical six-target list"

echo "==> check-release-targets passes on repo"
bash "$CHECK"
echo "ok: completeness check"

echo "==> unsupported optionalDependency is rejected (fixture)"
python3 - <<'PY'
import json
import os
import subprocess
import tempfile
from pathlib import Path

root = Path(".").resolve()
pkg = root / "packages/bindings/package.json"
original = pkg.read_text(encoding="utf-8")
try:
    data = json.loads(original)
    data["optionalDependencies"]["@node-webrtc-rust/bindings-linux-arm64-musl"] = "0.0.0-test"
    pkg.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    proc = subprocess.run(
        ["bash", "scripts/ci/check-release-targets.sh"],
        cwd=root,
        capture_output=True,
        text=True,
    )
    if proc.returncode == 0:
        raise SystemExit("FAIL: check accepted unsupported optionalDependency")
    if "optionalDependencies must be exactly" not in (proc.stderr + proc.stdout):
        raise SystemExit(f"FAIL: unexpected output: {proc.stderr}\n{proc.stdout}")
    print("ok: unsupported optionalDependency rejected")
finally:
    pkg.write_text(original, encoding="utf-8")
PY

echo "==> loader may advertise fallbacks beyond the six (not a release-contract failure)"
# index.js requires android/freebsd/etc. — check must still pass (already did above).
if ! rg -q "bindings-android-arm64" packages/bindings/index.js; then
  echo "FAIL: expected loader fallback require for sanity of this test" >&2
  exit 1
fi
bash "$CHECK" >/dev/null
echo "ok: loader fallbacks do not break release-target check"

echo "check-release-targets.test.sh: all checks passed"
