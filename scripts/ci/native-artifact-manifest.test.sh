#!/usr/bin/env bash
# Fail-closed tests for native artifact provenance manifests.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

MAN="$ROOT/scripts/ci/native-artifact-manifest.sh"
PY="$ROOT/scripts/ci/native_build_contract.py"
chmod +x "$MAN"

TARGET=x86_64-unknown-linux-gnu
PROFILE=release

# Prefer a real .node if present; otherwise synthesize a tiny fixture binary.
NODE_FIXTURE="$(mktemp "${TMPDIR:-/tmp}/native-manifest-XXXXXX.node")"
cleanup() {
  rm -f "$NODE_FIXTURE" "$MANIFEST" "$MANIFEST_BAD" 2>/dev/null || true
}
MANIFEST="$(mktemp "${TMPDIR:-/tmp}/native-manifest-XXXXXX.json")"
MANIFEST_BAD="$(mktemp "${TMPDIR:-/tmp}/native-manifest-bad-XXXXXX.json")"
trap cleanup EXIT

REAL_NODE=""
shopt -s nullglob
for candidate in \
  "packages/bindings/node-webrtc-rust.linux-x64-gnu.node" \
  "packages/bindings/node-webrtc-rust.darwin-arm64.node" \
  "packages/bindings/node-webrtc-rust.darwin-x64.node" \
  "packages/bindings/node-webrtc-rust.node"
do
  if [[ -f "$candidate" ]]; then
    REAL_NODE="$candidate"
    break
  fi
done
shopt -u nullglob

if [[ -n "$REAL_NODE" ]]; then
  # Use host-matching target when possible for produce path auto-discover tests.
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) TARGET=aarch64-apple-darwin ;;
    Darwin-x86_64) TARGET=x86_64-apple-darwin ;;
    Linux-x86_64) TARGET=x86_64-unknown-linux-gnu ;;
  esac
  cp "$REAL_NODE" "$NODE_FIXTURE"
else
  printf 'fake-native-binding-for-manifest-tests\n' >"$NODE_FIXTURE"
fi

echo "==> produce manifest"
bash "$MAN" produce \
  --target "$TARGET" \
  --profile "$PROFILE" \
  --output "$MANIFEST" \
  --node "$NODE_FIXTURE"
python3 - <<PY
import json, sys
m = json.load(open("$MANIFEST", encoding="utf-8"))
for key in ("schema", "target", "profile", "features", "input_digest", "node_artifact", "napi_surface_digest"):
    assert key in m, key
assert m["target"] == "$TARGET"
assert len(m["input_digest"]) == 64
assert len(m["node_artifact"]["sha256"]) == 64
assert m["node_artifact"]["size"] > 0
print("ok: manifest fields present")
PY

echo "==> napi_surface_digest matches committed surface at produce time"
python3 - <<PY
import json
import subprocess
import sys
sys.path.insert(0, "scripts/ci")
from pathlib import Path
import native_build_contract as nbc

root = Path(".")
manifest = json.loads(Path("$MANIFEST").read_text(encoding="utf-8"))
expected = nbc.napi_surface_digest(root)
if manifest["napi_surface_digest"] != expected:
    raise SystemExit(
        "napi_surface_digest must match HEAD index.js/index.d.ts at produce time"
    )
# Simulate post-napi drift then produce via write script path (restores first).
index = Path("packages/bindings/index.js")
original = index.read_text(encoding="utf-8")
index.write_text(original + "\n// produce-time drift probe\n", encoding="utf-8")
try:
    drifted = nbc.napi_surface_digest(root)
    if drifted == expected:
        raise SystemExit("drift probe did not change napi_surface_digest")
    subprocess.run(
        ["bash", "scripts/ci/restore-bindings-napi-surface-from-head.sh"],
        check=True,
    )
    restored = nbc.napi_surface_digest(root)
    if restored != expected:
        raise SystemExit("restore did not return napi_surface_digest to HEAD")
finally:
    index.write_text(original, encoding="utf-8")
print("ok: napi_surface_digest matches committed surface after restore")
PY

echo "==> validate accepts good manifest"
bash "$MAN" validate --manifest "$MANIFEST"
echo "ok: validate good manifest"

echo "==> reject checksum mismatch"
python3 - <<PY
import json
from pathlib import Path
p = Path("$MANIFEST")
m = json.loads(p.read_text(encoding="utf-8"))
m["node_artifact"]["sha256"] = "0" * 64
Path("$MANIFEST_BAD").write_text(json.dumps(m) + "\n", encoding="utf-8")
PY
if bash "$MAN" validate --manifest "$MANIFEST_BAD" 2>/tmp/native-manifest-validate.err; then
  echo "FAIL: validate accepted mismatched sha256" >&2
  exit 1
fi
rg -q "sha256 mismatch" /tmp/native-manifest-validate.err
echo "ok: checksum mismatch rejected"

echo "==> reject malformed schema"
python3 - <<PY
import json
from pathlib import Path
m = json.loads(Path("$MANIFEST").read_text(encoding="utf-8"))
m["schema"] = "not-a-real-schema"
Path("$MANIFEST_BAD").write_text(json.dumps(m) + "\n", encoding="utf-8")
PY
if bash "$MAN" validate --manifest "$MANIFEST_BAD" 2>/tmp/native-manifest-validate.err; then
  echo "FAIL: validate accepted bad schema" >&2
  exit 1
fi
rg -q "unsupported schema" /tmp/native-manifest-validate.err
echo "ok: bad schema rejected"

echo "==> reject features/profile contract mismatch"
python3 - <<PY
import json
from pathlib import Path
m = json.loads(Path("$MANIFEST").read_text(encoding="utf-8"))
m["features"] = ["silero-vad"]
Path("$MANIFEST_BAD").write_text(json.dumps(m) + "\n", encoding="utf-8")
PY
if bash "$MAN" validate --manifest "$MANIFEST_BAD" 2>/tmp/native-manifest-validate.err; then
  echo "FAIL: validate accepted feature mismatch" >&2
  exit 1
fi
rg -q "features mismatch" /tmp/native-manifest-validate.err
echo "ok: feature mismatch rejected"

echo "==> accept distribution-only drift with flag"
python3 - <<PY
import json
from pathlib import Path
m = json.loads(Path("$MANIFEST").read_text(encoding="utf-8"))
m["distribution_digest"] = "0" * 64
Path("$MANIFEST_BAD").write_text(json.dumps(m) + "\n", encoding="utf-8")
PY
if bash "$MAN" validate --manifest "$MANIFEST_BAD" 2>/tmp/native-manifest-validate.err; then
  echo "FAIL: validate accepted distribution drift without flag" >&2
  exit 1
fi
rg -q "distribution_digest mismatch" /tmp/native-manifest-validate.err
bash "$MAN" validate --manifest "$MANIFEST_BAD" --allow-distribution-drift
echo "ok: distribution drift accepted with flag"

echo "==> reject truncated / non-hex digest"
python3 - <<PY
import json
from pathlib import Path
m = json.loads(Path("$MANIFEST").read_text(encoding="utf-8"))
m["input_digest"] = "deadbeef"
Path("$MANIFEST_BAD").write_text(json.dumps(m) + "\n", encoding="utf-8")
PY
if bash "$MAN" validate --manifest "$MANIFEST_BAD" --allow-missing-node --skip-recompute \
  2>/tmp/native-manifest-validate.err; then
  echo "FAIL: validate accepted short digest" >&2
  exit 1
fi
rg -q "input_digest must be 64" /tmp/native-manifest-validate.err
echo "ok: malformed digest rejected"

echo "native-artifact-manifest.test.sh: all checks passed"
