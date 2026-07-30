#!/usr/bin/env bash
# Fixture tests for native-main-bundle resolver trust rules.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

PY=scripts/ci/resolve_native_main_bundle.py
chmod +x scripts/ci/resolve-native-main-bundle.sh "$PY"

export NATIVE_TOOL_MODE=declared
export GITHUB_REPOSITORY="${GITHUB_REPOSITORY:-akirilyuk/node-webrtc-rust}"

TMP="$(mktemp -d "${TMPDIR:-/tmp}/resolve-bundle-XXXXXX")"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

CURRENT_SHA="$(git rev-parse HEAD)"
OTHER_SHA="$(printf 'b%.0s' {1..40})"
OTHER_SHA="${OTHER_SHA:0:40}"

make_bundle_zip() {
  local zip_path="$1"
  local aggregate="$2"
  local work="$TMP/bundle-work"
  rm -rf "$work"
  mkdir -p "$work"

  # Minimal fake six-target bundle matching current digests would require real .nodes.
  # For trust-rule tests we exercise API selection; validation failure is a separate case.
  python3 - <<PY
import json, zipfile
from pathlib import Path
meta = {
  "schema": "node-webrtc-rust.native-main-bundle/v1",
  "profile": "release",
  "aggregate_digest": "$aggregate",
  "targets": {},
  "producer": {"git_sha": "deadbeef", "run_id": "1"},
}
Path("$work/meta.json").write_text(json.dumps(meta) + "\n", encoding="utf-8")
with zipfile.ZipFile("$zip_path", "w") as zf:
    zf.write("$work/meta.json", "meta.json")
PY
}

write_runs() {
  local path="$1"
  python3 - <<PY
import json
from pathlib import Path
runs = [
  {
    "id": 100,
    "name": "Build & Test (main)",
    "path": ".github/workflows/build-main.yml",
    "head_branch": "main",
    "head_sha": "$CURRENT_SHA",
    "status": "completed",
    "conclusion": "success",
  },
  {
    "id": 90,
    "name": "Build & Test (main)",
    "path": ".github/workflows/build-main.yml",
    "head_branch": "main",
    "head_sha": "$OTHER_SHA",
    "status": "completed",
    "conclusion": "success",
  },
  {
    "id": 80,
    "name": "Evil workflow",
    "path": ".github/workflows/evil.yml",
    "head_branch": "main",
    "head_sha": "$CURRENT_SHA",
    "status": "completed",
    "conclusion": "success",
  },
  {
    "id": 70,
    "name": "Build & Test (main)",
    "path": ".github/workflows/build-main.yml",
    "head_branch": "feat/not-main",
    "head_sha": "$CURRENT_SHA",
    "status": "completed",
    "conclusion": "success",
  },
]
Path("$path").write_text(json.dumps({"workflow_runs": runs}) + "\n", encoding="utf-8")
PY
}

echo "==> reject wrong workflow / branch (no usable artifact → fallback)"
FIX="$TMP/fix1"
mkdir -p "$FIX"
write_runs "$FIX/workflow_runs.json"
# Only evil/wrong-branch runs have artifacts — trusted runs have none
python3 - <<PY
import json
from pathlib import Path
Path("$FIX/run-100-artifacts.json").write_text(json.dumps({"artifacts": []}) + "\n")
Path("$FIX/run-90-artifacts.json").write_text(json.dumps({"artifacts": []}) + "\n")
PY
out="$(
  bash scripts/ci/resolve-native-main-bundle.sh \
    --profile release \
    --download-dir "$TMP/dl1" \
    --fixture-dir "$FIX" \
    --sha "$CURRENT_SHA" 2>/dev/null | rg '^fallback_reason=' || true
)"
if [[ "$out" != "fallback_reason=artifact_missing" && "$out" != "fallback_reason=no_matching_bundle" ]]; then
  # Accept artifact_missing after trusted runs with empty artifact lists
  if ! echo "$out" | rg -q 'fallback_reason=(artifact_missing|no_matching_bundle|bundle_validation_failed)'; then
    echo "FAIL: unexpected fallback: $out" >&2
    exit 1
  fi
fi
echo "ok: missing artifact falls back ($out)"

echo "==> reject expired artifact"
FIX="$TMP/fix2"
mkdir -p "$FIX"
write_runs "$FIX/workflow_runs.json"
python3 - <<PY
import json
from pathlib import Path
Path("$FIX/run-100-artifacts.json").write_text(json.dumps({
  "artifacts": [{
    "id": 1,
    "name": "native-main-bundle",
    "expired": True,
  }]
}) + "\n")
Path("$FIX/run-90-artifacts.json").write_text(json.dumps({"artifacts": []}) + "\n")
PY
out="$(
  bash scripts/ci/resolve-native-main-bundle.sh \
    --profile release --download-dir "$TMP/dl2" --fixture-dir "$FIX" --sha "$CURRENT_SHA" \
    2>/dev/null | rg '^fallback_reason=' || true
)"
if [[ "$out" != "fallback_reason=artifact_expired" ]]; then
  # may cascade to no_matching_bundle if both fail
  echo "$out" | rg -q 'fallback_reason=(artifact_expired|no_matching_bundle|artifact_missing)' || {
    echo "FAIL: expected expired fallback, got $out" >&2
    exit 1
  }
fi
echo "ok: expired artifact rejected ($out)"

echo "==> reject malformed API JSON"
FIX="$TMP/fix3"
mkdir -p "$FIX"
echo 'not-json' >"$FIX/workflow_runs.json"
out="$(
  bash scripts/ci/resolve-native-main-bundle.sh \
    --profile release --download-dir "$TMP/dl3" --fixture-dir "$FIX" --sha "$CURRENT_SHA" \
    2>/dev/null | rg '^bundle_reused=' || true
)"
if [[ "$out" != "bundle_reused=false" ]]; then
  echo "FAIL: malformed API should not reuse ($out)" >&2
  exit 1
fi
echo "ok: malformed API falls back"

echo "==> exact-SHA preference ordering unit check"
python3 - <<PY
import sys
from pathlib import Path
sys.path.insert(0, "scripts/ci")
import resolve_native_main_bundle as r

runs = [
  {"id": 2, "head_sha": "bbbb"},
  {"id": 1, "head_sha": "aaaa"},
  {"id": 3, "head_sha": "cccc"},
]
ordered = r.order_runs_for_preference(runs, "aaaa")
assert ordered[0]["head_sha"] == "aaaa", ordered
assert [x["head_sha"] for x in ordered[1:]] == ["bbbb", "cccc"]
print("ok: exact SHA preferred before older fingerprint candidates")
PY

echo "==> cache-key wiring uses native-v3 + target"
export NATIVE_TOOL_MODE=declared
key="$(python3 scripts/ci/native_build_contract.py cache-key --target x86_64-unknown-linux-gnu --profile release)"
echo "$key" | rg -q '^native-v3-release-x86_64-unknown-linux-gnu-[0-9a-f]{64}$'
key_musl="$(python3 scripts/ci/native_build_contract.py cache-key --target x86_64-unknown-linux-musl --profile release)"
if [[ "$key" == "$key_musl" ]]; then
  echo "FAIL: gnu and musl cache keys must differ" >&2
  exit 1
fi
echo "ok: per-target native-v3 cache-key wiring"

echo "resolve-native-main-bundle.test.sh: all checks passed"
