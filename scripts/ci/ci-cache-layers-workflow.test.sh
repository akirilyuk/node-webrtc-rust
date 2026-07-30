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
summary=scripts/ci/write-native-ci-summary.sh

# --- Plan runner: cargo metadata is required on bare self-hosted hosts ---
grep -q 'dtolnay/rust-toolchain@stable' "$plan" || fail "plan action must install Cargo for metadata"
toolchain_line="$(grep -n 'dtolnay/rust-toolchain@stable' "$plan" | cut -d: -f1)"
plan_line="$(grep -n 'name: Plan per-target native builds' "$plan" | cut -d: -f1)"
[[ "$toolchain_line" -lt "$plan_line" ]] || fail "Rust toolchain must be installed before native planning"
echo "ok: plan installs Cargo before fingerprint metadata"

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
