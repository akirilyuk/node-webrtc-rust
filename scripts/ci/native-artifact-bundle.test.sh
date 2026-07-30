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
  --artifacts-root "$ART" --output "$OUT" --profile release 2>"$TMP/bundle-incomplete.err"; then
  echo "FAIL: assemble accepted incomplete target set" >&2
  exit 1
fi
rg -q 'missing directory' "$TMP/bundle-incomplete.err"
echo "ok: incomplete assemble rejected"

echo "==> noncanonical target binary rejected"
mv \
  "$ART/bindings-x86_64-unknown-linux-gnu/node-webrtc-rust.linux-x64-gnu.node" \
  "$ART/bindings-x86_64-unknown-linux-gnu/wrong-target.node"
if bash scripts/ci/native-artifact-bundle.sh assemble \
  --artifacts-root "$ART" --output "$OUT" --profile release 2>"$TMP/bundle-canonical.err"; then
  echo "FAIL: assemble accepted a noncanonical target binary" >&2
  exit 1
fi
rg -q 'canonical .node missing' "$TMP/bundle-canonical.err"
rm -f "$ART/bindings-x86_64-unknown-linux-gnu/wrong-target.node"
echo "ok: canonical target filename required"

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
  # Real build artifacts record a producer-workspace path that does not exist in
  # the later assembly job. The bundle must validate its copied bytes instead.
  python3 - <<PY
import json
from pathlib import Path
p = Path("$dir/manifest.json")
m = json.loads(p.read_text(encoding="utf-8"))
m["node_artifact"]["path"] = "packages/bindings/producer-only/$base"
p.write_text(json.dumps(m) + "\n", encoding="utf-8")
PY
done < <(bash scripts/ci/list-release-targets.sh)

rm -rf "$OUT"
NATIVE_TOOL_MODE=unresolved bash scripts/ci/native-artifact-bundle.sh assemble \
  --artifacts-root "$ART" --output "$OUT" --profile release >"$TMP/bundle-agg.txt"
agg="$(<"$TMP/bundle-agg.txt")"
NATIVE_TOOL_MODE=unresolved bash scripts/ci/native-artifact-bundle.sh validate \
  --bundle "$OUT" --profile release
echo "ok: full six-target bundle validate (aggregate=$agg)"

echo "==> copied bundle bytes are authoritative"
printf 'tamper\n' >>"$OUT/x86_64-unknown-linux-gnu/node-webrtc-rust.linux-x64-gnu.node"
if NATIVE_TOOL_MODE=unresolved bash scripts/ci/native-artifact-bundle.sh validate \
  --bundle "$OUT" --profile release 2>"$TMP/bundle-node-tamper.err"; then
  echo "FAIL: validate accepted tampered bundled .node via stale producer path" >&2
  exit 1
fi
rg -q 'sha256 mismatch' "$TMP/bundle-node-tamper.err"
echo "ok: bundled .node tamper rejected"

echo "==> checksum tamper rejected"
rm -rf "$OUT"
NATIVE_TOOL_MODE=unresolved bash scripts/ci/native-artifact-bundle.sh assemble \
  --artifacts-root "$ART" --output "$OUT" --profile release >/dev/null
python3 - <<PY
import json
from pathlib import Path
p = Path("$OUT/x86_64-unknown-linux-gnu/manifest.json")
m = json.loads(p.read_text(encoding="utf-8"))
m["node_artifact"]["sha256"] = "0" * 64
p.write_text(json.dumps(m) + "\n", encoding="utf-8")
PY
if NATIVE_TOOL_MODE=unresolved bash scripts/ci/native-artifact-bundle.sh validate \
  --bundle "$OUT" --profile release 2>"$TMP/bundle-tamper.err"; then
  echo "FAIL: validate accepted tampered checksum" >&2
  exit 1
fi
rg -q 'sha256 mismatch|validate-bundle|validate-manifest' "$TMP/bundle-tamper.err"
echo "ok: tampered checksum rejected"

echo "==> bundle metadata drift rejected"
rm -rf "$OUT"
NATIVE_TOOL_MODE=unresolved bash scripts/ci/native-artifact-bundle.sh assemble \
  --artifacts-root "$ART" --output "$OUT" --profile release >/dev/null
python3 - <<PY
import json
from pathlib import Path
p = Path("$OUT/meta.json")
m = json.loads(p.read_text(encoding="utf-8"))
m["targets"]["x86_64-unknown-linux-gnu"]["node_sha256"] = "0" * 64
p.write_text(json.dumps(m) + "\n", encoding="utf-8")
PY
if NATIVE_TOOL_MODE=unresolved bash scripts/ci/native-artifact-bundle.sh validate \
  --bundle "$OUT" --profile release 2>"$TMP/bundle-meta-tamper.err"; then
  echo "FAIL: validate accepted metadata/manifest checksum drift" >&2
  exit 1
fi
rg -q 'meta/manifest node sha256 drift' "$TMP/bundle-meta-tamper.err"
echo "ok: metadata/manifest drift rejected"

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

echo "==> metadata cannot redirect staged paths"
BAD="$TMP/bad-bundle"
cp -R "$OUT" "$BAD"
python3 - <<PY
import json
from pathlib import Path
p = Path("$BAD/meta.json")
m = json.loads(p.read_text(encoding="utf-8"))
m["targets"]["x86_64-unknown-linux-gnu"]["node_basename"] = "../../escape.node"
p.write_text(json.dumps(m) + "\n", encoding="utf-8")
PY
if NATIVE_TOOL_MODE=unresolved python3 scripts/ci/native_build_contract.py stage-bundle \
  --bundle "$BAD" --output "$TMP/bad-stage" 2>"$TMP/bundle-stage-tamper.err"; then
  echo "FAIL: stage accepted tampered node_basename" >&2
  exit 1
fi
rg -q 'node_basename mismatch' "$TMP/bundle-stage-tamper.err"
test ! -e "$TMP/escape.node"
echo "ok: staged paths use canonical target mapping"

echo "native-artifact-bundle.test.sh: all checks passed"
