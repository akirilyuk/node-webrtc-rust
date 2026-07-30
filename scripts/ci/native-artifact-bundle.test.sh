#!/usr/bin/env bash
# Bundle assemble/validate completeness tests (synthetic six-target fixtures).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

export NATIVE_TOOL_MODE=unresolved
chmod +x scripts/ci/native-artifact-bundle.sh

TMP="$(mktemp -d "${TMPDIR:-/tmp}/native-bundle-XXXXXX")"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

ART="$TMP/artifacts"
OUT="$TMP/bundle"
mkdir -p "$ART"

echo "==> incomplete bundle rejected"
mkdir -p "$ART/bindings-x86_64-unknown-linux-gnu"
printf 'partial\n' >"$ART/bindings-x86_64-unknown-linux-gnu/node-webrtc-rust.linux-x64-gnu.node"
if bash scripts/ci/native-artifact-bundle.sh assemble \
  --artifacts-root "$ART" --output "$OUT" --profile release 2>/tmp/bundle-incomplete.err; then
  echo "FAIL: assemble accepted incomplete target set" >&2
  exit 1
fi
rg -q 'missing directory' /tmp/bundle-incomplete.err
echo "ok: incomplete assemble rejected"

echo "==> full synthetic bundle assembles + validates"
# Build six tiny .node fixtures and manifests via produce-manifest.
while IFS= read -r target; do
  [[ -z "$target" ]] && continue
  dir="$ART/bindings-${target}"
  mkdir -p "$dir"
  base="$(python3 - <<PY
import sys
sys.path.insert(0, "scripts/ci")
import native_build_contract as nbc
print(nbc.RELEASE_TARGET_MAP["$target"]["node_basename"])
PY
)"
  printf 'synthetic-node-for-%s\n' "$target" >"$dir/$base"
  # Produce manifest against this node
  NATIVE_TOOL_MODE=unresolved python3 scripts/ci/native_build_contract.py produce-manifest \
    --target "$target" --profile release --output "$dir/manifest.json" --node "$dir/$base"
done < <(bash scripts/ci/list-release-targets.sh)

rm -rf "$OUT"
NATIVE_TOOL_MODE=unresolved bash scripts/ci/native-artifact-bundle.sh assemble \
  --artifacts-root "$ART" --output "$OUT" --profile release >/tmp/bundle-agg.txt
agg="$(cat /tmp/bundle-agg.txt)"
NATIVE_TOOL_MODE=unresolved bash scripts/ci/native-artifact-bundle.sh validate \
  --bundle "$OUT" --profile release
echo "ok: full six-target bundle validate (aggregate=$agg)"

echo "==> checksum tamper rejected"
python3 - <<PY
import json
from pathlib import Path
p = Path("$OUT/x86_64-unknown-linux-gnu/manifest.json")
m = json.loads(p.read_text(encoding="utf-8"))
m["node_artifact"]["sha256"] = "0" * 64
p.write_text(json.dumps(m) + "\n", encoding="utf-8")
PY
if NATIVE_TOOL_MODE=unresolved bash scripts/ci/native-artifact-bundle.sh validate \
  --bundle "$OUT" --profile release 2>/tmp/bundle-tamper.err; then
  echo "FAIL: validate accepted tampered checksum" >&2
  exit 1
fi
rg -q 'sha256 mismatch|validate-bundle|validate-manifest' /tmp/bundle-tamper.err
echo "ok: tampered checksum rejected"

echo "==> stage-bundle produces bindings-* dirs"
STAGE="$TMP/stage"
# restore a good bundle first
rm -rf "$OUT"
NATIVE_TOOL_MODE=unresolved bash scripts/ci/native-artifact-bundle.sh assemble \
  --artifacts-root "$ART" --output "$OUT" --profile release >/dev/null
NATIVE_TOOL_MODE=unresolved python3 scripts/ci/native_build_contract.py stage-bundle \
  --bundle "$OUT" --output "$STAGE"
test -f "$STAGE/bindings-x86_64-unknown-linux-gnu/manifest.json"
test -f "$STAGE/bindings-x86_64-pc-windows-msvc/"*.node
echo "ok: stage-bundle layout"

echo "native-artifact-bundle.test.sh: all checks passed"
