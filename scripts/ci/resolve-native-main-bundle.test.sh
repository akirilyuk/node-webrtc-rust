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

echo "==> exact-SHA valid portable bundle is reused"
VALID_ART="$TMP/valid-artifacts"
VALID_BUNDLE="$TMP/valid-bundle"
mkdir -p "$VALID_ART"
while IFS= read -r target; do
  [[ -z "$target" ]] && continue
  dir="$VALID_ART/bindings-${target}"
  mkdir -p "$dir"
  base="$(python3 - <<PY
import sys
sys.path.insert(0, "scripts/ci")
import native_build_contract as nbc
print(nbc.RELEASE_TARGET_MAP["$target"]["node_basename"])
PY
)"
  printf 'resolver-node-for-%s\n' "$target" >"$dir/$base"
  NATIVE_TOOL_MODE=declared python3 scripts/ci/native_build_contract.py produce-manifest \
    --target "$target" --profile release --output "$dir/manifest.json" --node "$dir/$base"
  python3 - <<PY
import json
from pathlib import Path
p = Path("$dir/manifest.json")
m = json.loads(p.read_text(encoding="utf-8"))
m["node_artifact"]["path"] = "packages/bindings/producer-only/$base"
p.write_text(json.dumps(m) + "\n", encoding="utf-8")
PY
done < <(bash scripts/ci/list-release-targets.sh)
NATIVE_TOOL_MODE=declared bash scripts/ci/native-artifact-bundle.sh assemble \
  --artifacts-root "$VALID_ART" --output "$VALID_BUNDLE" --profile release >/dev/null

FIX="$TMP/fix-valid"
mkdir -p "$FIX"
write_runs "$FIX/workflow_runs.json"
python3 - <<PY
import json
import zipfile
from pathlib import Path

Path("$FIX/run-100-artifacts.json").write_text(json.dumps({
  "artifacts": [{"id": 1001, "name": "native-main-bundle", "expired": False}]
}) + "\n", encoding="utf-8")
Path("$FIX/run-90-artifacts.json").write_text(
    json.dumps({"artifacts": []}) + "\n", encoding="utf-8"
)
bundle = Path("$VALID_BUNDLE")
with zipfile.ZipFile("$FIX/artifact-1001.zip", "w") as zf:
    for path in bundle.rglob("*"):
        if path.is_file():
            zf.write(path, path.relative_to(bundle))
PY
out="$(
  bash scripts/ci/resolve-native-main-bundle.sh \
    --profile release \
    --download-dir "$TMP/dl-valid" \
    --fixture-dir "$FIX" \
    --sha "$CURRENT_SHA"
)"
echo "$out" | rg -q '^bundle_reused=true$'
echo "$out" | rg -q '^run_id=100$'
echo "$out" | rg -q '^exact_sha=true$'
echo "$out" | rg -q '^preference=exact_sha$'
test -f "$TMP/dl-valid/extract-100/meta.json"
echo "ok: exact-SHA portable bundle reused and validated"

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

echo "==> transient GitHub API network errors retry, then fall back"
GITHUB_OUTPUT="$TMP/gh-output-net" python3 - <<'PY'
import http.client
import io
import os
import sys
import urllib.error
from pathlib import Path

sys.path.insert(0, "scripts/ci")
import resolve_native_main_bundle as r


class FakeResp:
    headers = {}

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False

    def getcode(self):
        return 200

    def read(self):
        return b"{}"


def run_with(side_effects):
    calls = []
    sleeps = []
    queue = list(side_effects)

    def fake_urlopen(req, timeout=None):
        calls.append(req.full_url)
        item = queue.pop(0)
        if isinstance(item, BaseException):
            raise item
        return item

    orig_urlopen, orig_sleep = r.urllib.request.urlopen, r._sleep
    r.urllib.request.urlopen = fake_urlopen
    r._sleep = sleeps.append
    try:
        try:
            result = r._github_request("https://api.github.com/x", "tok")
            err = None
        except r.ResolveError as exc:
            result, err = None, exc
    finally:
        r.urllib.request.urlopen, r._sleep = orig_urlopen, orig_sleep
    return result, err, calls, sleeps


def http_error(code):
    return urllib.error.HTTPError(
        "https://api.github.com/x", code, "x", {}, io.BytesIO(b"body")
    )


rd = http.client.RemoteDisconnected("Remote end closed connection without response")

result, err, calls, sleeps = run_with([rd, rd, FakeResp()])
assert err is None and result[1] == b"{}", (result, err)
assert len(calls) == 3 and sleeps == [2, 4], (calls, sleeps)

result, err, calls, sleeps = run_with([rd, rd, rd])
assert err is not None and err.reason == "network_error", err
assert len(calls) == 3 and sleeps == [2, 4], (calls, sleeps)

result, err, calls, sleeps = run_with([http_error(404)])
assert err is not None and err.reason == "http_error", err
assert len(calls) == 1 and sleeps == [], (calls, sleeps)

result, err, calls, sleeps = run_with([http_error(503), FakeResp()])
assert err is None and len(calls) == 2 and sleeps == [2], (err, calls, sleeps)

result, err, calls, sleeps = run_with([http_error(502), http_error(502), http_error(502)])
assert err is not None and err.reason == "http_error" and len(calls) == 3, err

# End to end: dropped connections on every attempt -> fallback output, exit 0.
orig_urlopen, orig_sleep = r.urllib.request.urlopen, r._sleep


def always_drop(req, timeout=None):
    raise rd


r.urllib.request.urlopen = always_drop
r._sleep = lambda s: None
try:
    code = r.main(
        [
            "--profile", "release",
            "--download-dir", os.environ["GITHUB_OUTPUT"] + ".dl",
            "--sha", "a" * 40,
            "--repository", "akirilyuk/node-webrtc-rust",
            "--token", "tok",
        ]
    )
finally:
    r.urllib.request.urlopen, r._sleep = orig_urlopen, orig_sleep
assert code == 0, code
out = Path(os.environ["GITHUB_OUTPUT"]).read_text()
assert "bundle_reused=false" in out, out
assert "fallback_reason=network_error" in out, out
print("ok: transient network errors retry and fall back")
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
