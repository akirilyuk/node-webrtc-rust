#!/usr/bin/env bash
# Static checks for Cargo/Docker/summary/smoke workflow contracts.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

fail() { echo "FAIL: $*" >&2; exit 1; }

linux=".github/actions/ci-build-native-linux/action.yml"
host=".github/actions/ci-build-native-host/action.yml"
image=".github/workflows/ci-image.yml"
smoke=".github/workflows/native-cache-smoke.yml"
plan=".github/actions/plan-native-builds/action.yml"
stage_cached=".github/workflows/reusable-stage-cached-bindings.yml"
main=".github/workflows/build-main.yml"
release=".github/workflows/release.yml"
pr=".github/workflows/build.yml"
compile_recipe="scripts/ci/build-native-addon.sh"
assert_surface="scripts/ci/assert-bindings-napi-surface-clean.sh"
manifest_write="scripts/ci/write-native-artifact-manifest.sh"
summary=scripts/ci/write-native-ci-summary.sh

# --- Plan runner: cargo metadata is required on bare self-hosted hosts ---
grep -q 'dtolnay/rust-toolchain@stable' "$plan" || fail "plan action must install Cargo for metadata"
toolchain_line="$(grep -n 'dtolnay/rust-toolchain@stable' "$plan" | cut -d: -f1)"
plan_line="$(grep -n 'name: Plan per-target native builds' "$plan" | cut -d: -f1)"
[[ "$toolchain_line" -lt "$plan_line" ]] || fail "Rust toolchain must be installed before native planning"
echo "ok: plan installs Cargo before fingerprint metadata"

# Every bare self-hosted job that computes or validates the native contract must
# install Cargo before its contract step. GitHub-hosted native build jobs already
# provide/install their target toolchains.
python3 - "$main" "$release" "$smoke" <<'PY' || fail "bare native-contract job missing Cargo setup"
from pathlib import Path
import re
import sys


def job(text: str, name: str) -> str:
    match = re.search(
        rf"(?ms)^  {re.escape(name)}:\n(.*?)(?=^  [a-zA-Z0-9_-]+:\n|\Z)",
        text,
    )
    if not match:
        raise SystemExit(f"missing workflow job: {name}")
    return match.group(1)


def require_toolchain_before(path: str, job_name: str, marker: str) -> None:
    block = job(Path(path).read_text(encoding="utf-8"), job_name)
    toolchain = block.find("dtolnay/rust-toolchain@stable")
    contract = block.find(marker)
    if contract < 0:
        raise SystemExit(f"{path}:{job_name}: missing contract marker {marker!r}")
    if toolchain < 0 or toolchain > contract:
        raise SystemExit(
            f"{path}:{job_name}: Rust toolchain must precede {marker!r}"
        )


main, release, smoke = sys.argv[1:]
require_toolchain_before(main, "assemble-native-bundle", "native-artifact-bundle.sh assemble")
require_toolchain_before(release, "plan", "resolve-native-main-bundle.sh")
require_toolchain_before(release, "reuse-bundle", "native-artifact-bundle.sh validate")
require_toolchain_before(smoke, "assemble-smoke-bundle", "native-artifact-bundle.sh assemble")
require_toolchain_before(smoke, "resolve-release-style", "resolve-native-main-bundle.sh")
PY
echo "ok: all bare native-contract jobs install Cargo first"

grep -q 'allow-distribution-drift' .github/actions/native-binding-cache/action.yml \
  || fail "native-binding-cache must tolerate distribution drift on restore"
grep -q 'dtolnay/rust-toolchain@stable' "$stage_cached" \
  || fail "stage-cached workflow must install Cargo before manifest refresh"
python3 - "$stage_cached" <<'PY' || fail "stage-cached must install Cargo before write-native-artifact-manifest"
from pathlib import Path
import re
import sys

text = Path(sys.argv[1]).read_text(encoding="utf-8")
for job in ("stage-linux-x64", "stage-linux-arm64", "stage-host"):
    match = re.search(
        rf"(?ms)^  {re.escape(job)}:\n(.*?)(?=^  [a-zA-Z0-9_-]+:\n|\Z)",
        text,
    )
    if not match:
        raise SystemExit(f"missing job: {job}")
    block = match.group(1)
    toolchain = block.find("dtolnay/rust-toolchain@stable")
    manifest = block.find("write-native-artifact-manifest.sh")
    if manifest < 0:
        raise SystemExit(f"{job}: missing manifest refresh")
    if toolchain < 0 or toolchain > manifest:
        raise SystemExit(f"{job}: Rust toolchain must precede manifest refresh")
PY
echo "ok: stage-cached jobs install Cargo before manifest refresh"

grep -q 'Install Rust metadata toolchain' "$host" \
  || fail "host build must install Cargo metadata for manifest refresh"
grep -q "build_required != 'true'" "$host" \
  || fail "host build must install metadata toolchain on cache-hit skip path"
echo "ok: host cache-hit path installs Cargo before manifest write"

# --- Compile recipe: only byte-affecting inputs invalidate native artifacts ---
grep -q "$compile_recipe" "$linux" || fail "linux build must delegate to canonical compile recipe"
grep -q "$compile_recipe" "$host" || fail "host build must delegate to canonical compile recipe"
if grep -qE '\bnpx napi build\b' "$linux" "$host"; then
  fail "native compile commands must live only in the fingerprinted recipe script"
fi
grep -q "$assert_surface" "$linux" || fail "linux build must assert napi surface matches HEAD before manifest"
grep -q "$assert_surface" "$host" || fail "host build must assert napi surface matches HEAD before manifest"
python3 - "$linux" "$host" <<'PY' || fail "assert surface step must follow build and precede manifest write"
from pathlib import Path
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
print("ok: napi surface assert between build and manifest")
PY
echo "ok: napi surface assert wired in linux/host actions"
if grep -qE 'inputs\.(platform|zig|sherpa_onnx_lib_dir|build_args)' "$linux" "$host"; then
  fail "compile-semantic inputs must be derived inside the fingerprinted recipe"
fi
if grep -q 'napi-zig\|napi-rs-nodejs' "$host" "$linux"; then
  fail "unused zig cache path must not remain (Path Validation on empty dir)"
fi
grep -q 'ensure-gnu-tar-alpine.sh' "$linux" || fail "musl linux build must ensure GNU tar before Actions cache"
grep -q 'ensure-gnu-tar-alpine.sh' .github/actions/native-binding-cache/action.yml \
  || fail "native-binding-cache must ensure GNU tar before musl restore"
python3 - "$pr" <<'PY' || fail "PR native path filter over-invalidates Rust builds"
from pathlib import Path
import re
import sys

text = Path(sys.argv[1]).read_text(encoding="utf-8")
match = re.search(
    r"(?ms)^\s{12}workflows_native:\n(.*?)(?=^\s{12}workflows_test:\n)",
    text,
)
if not match:
    raise SystemExit("workflows_native filter not found")
block = match.group(1)
required = (
    "scripts/ci/build-native-addon.sh",
    "scripts/ci/build-sherpa-onnx-musl-libs.sh",
    "scripts/ci/install-alpine-native-toolchain.sh",
    "scripts/ci/native-cache-epoch",
)
for path in required:
    if path not in block:
        raise SystemExit(f"compile-affecting path missing from workflows_native: {path}")
orchestration_only = (
    "scripts/ci/native_build_contract.py",
    "scripts/ci/native-build-fingerprint.sh",
    "scripts/ci/native-binding-cache-key.sh",
    "scripts/ci/native-artifact-",
    "scripts/ci/collect-native-tool-identity.sh",
    "scripts/ci/resolve-native-main-bundle",
    "scripts/ci/resolve_native_main_bundle.py",
    "scripts/ci/plan-native-builds.sh",
)
for path in orchestration_only:
    if path in block:
        raise SystemExit(f"orchestration-only path triggers native compile: {path}")
PY
echo "ok: canonical compile recipe is hashed; orchestration-only edits do not compile"

# --- Cargo: no broad restore-keys on target/; exact save present ---
if grep -A20 'Restore Cargo target' "$linux" | grep -q 'restore-keys:'; then
  fail "linux target/ restore must not use restore-keys"
fi
if ! grep -q 'Save Cargo target/' "$linux"; then
  fail "linux must save Cargo target/ on miss"
fi
if ! grep -q 'cargo-tgt-v1-' "$linux"; then
  fail "linux target cache key missing cargo-tgt-v1 namespace"
fi
# No duplicate Swatinem+actions/cache for same target tree on host
if grep -qE '^\s*path:\s*target\s*$' "$host"; then
  fail "host must not add separate actions/cache for target/ (Swatinem owns it)"
fi
echo "ok: Cargo target exact restore/save; host Swatinem-only for Cargo"

# --- Docker scopes + Alpine triggers ---
grep -q 'scope=ci-build-glibc' "$image" || fail "missing glibc BuildKit scope"
grep -q 'scope=ci-build-alpine' "$image" || fail "missing alpine BuildKit scope"
grep -q 'build-sherpa-onnx-musl-libs.sh' "$image" || fail "Alpine path trigger missing musl libs script"
grep -q 'ci-build:\${{ github.sha }}' "$image" || grep -q 'CI_IMAGE_SHA' "$image" || fail "missing SHA image tag"
grep -q 'content' "$image" || fail "image workflow should record content digest"
echo "ok: Docker scopes, Alpine triggers, SHA tags"

# Content digest script covers COPY'd Alpine inputs
digest_script=scripts/ci/ci-image-content-digest.sh
chmod +x "$digest_script"
g="$(bash "$digest_script" glibc)"
a="$(bash "$digest_script" alpine)"
[[ ${#g} -eq 64 && ${#a} -eq 64 && "$g" != "$a" ]] || fail "content digests invalid"
echo "ok: ci-image-content-digest glibc!=alpine"

# Declared tool identity must not use bare :latest as identity
if python3 - <<'PY'
import os, sys
sys.path.insert(0, "scripts/ci")
os.environ["NATIVE_TOOL_MODE"] = "declared"
os.environ.pop("CI_IMAGE", None)
os.environ.pop("CI_IMAGE_ALPINE", None)
import native_build_contract as nbc
img = nbc.declared_tool_identity("x86_64-unknown-linux-gnu")["image"]
alp = nbc.declared_tool_identity("x86_64-unknown-linux-musl")["image"]
print(img)
print(alp)
if img.endswith(":latest") or alp.endswith(":latest"):
    raise SystemExit("latest used as identity")
if "@content:" not in img and "@sha256:" not in img and ":latest" in img:
    raise SystemExit("unstable image ref")
if "@content:" not in alp and "@sha256:" not in alp:
    raise SystemExit("alpine missing content/digest identity")
PY
then
  echo "ok: declared image contract avoids bare :latest"
else
  fail "declared image contract still uses mutable latest"
fi

# --- Summary helper ---
[[ -x "$summary" || -f "$summary" ]] || fail "missing write-native-ci-summary.sh"
tmp="$(mktemp)"
GITHUB_STEP_SUMMARY="$tmp" SUMMARY_TITLE="t" AGGREGATE_DIGEST=abc \
  ALL_CACHED=false CACHED_TARGETS_JSON='[]' REBUILT_TARGETS_JSON='["x"]' \
  PRODUCER_SHA=deadbeef PRODUCER_RUN_ID=1 FALLBACK_REASON=miss \
  bash "$summary"
grep -q 'aggregate_digest' "$tmp" || fail "summary missing aggregate"
grep -q 'rebuilt_targets' "$tmp" || fail "summary missing rebuilt"
grep -q 'producer_sha' "$tmp" || fail "summary missing producer"
rm -f "$tmp"
echo "ok: GITHUB_STEP_SUMMARY helper"

# --- Smoke workflow safety ---
[[ -f "$smoke" ]] || fail "missing native-cache-smoke.yml"
grep -q 'workflow_dispatch' "$smoke" || fail "smoke must be workflow_dispatch"
if grep -qE '^\s+push:|^\s+pull_request:' "$smoke"; then
  fail "smoke must not trigger on push/PR"
fi
if grep -qiE '^\s*uses:\s*.*(softprops/action-gh-release|ncipollo/release-action|actions/upload-release)' "$smoke"; then
  fail "smoke must not contain publish/release actions"
fi
if grep -qiE '^\s+run:.*\bnpm\s+publish\b' "$smoke"; then
  fail "smoke must not run npm publish"
fi
if grep -qE 'name:\s*native-main-bundle' "$smoke"; then
  fail "smoke must not upload native-main-bundle (release trust boundary)"
fi
grep -q 'native-smoke-bundle' "$smoke" || fail "smoke should upload native-smoke-bundle"
grep -q 'reuse-check' "$smoke" || fail "smoke missing reuse-check phase"
grep -q 'all_cached' "$smoke" || fail "smoke missing all_cached assertion"
echo "ok: smoke workflow dispatch-only + no publish"

# plan emits rebuilt_targets
grep -q 'rebuilt_targets' scripts/ci/plan-native-builds.sh || fail "plan missing rebuilt_targets"
echo "ok: plan rebuilt_targets output"

echo "ci-cache-layers-workflow.test.sh: all checks passed"
