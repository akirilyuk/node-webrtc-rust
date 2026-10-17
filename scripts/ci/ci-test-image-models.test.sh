#!/usr/bin/env bash
# Guards ci.image-asset-lockstep for the ci-test image: every Sherpa model dir the
# integration suite expects is baked and checked in docker/ci/Dockerfile.test, coturn is
# installed, and ci-build (docker/ci/Dockerfile) stays free of test assets so its content
# digest (a native build fingerprint input) does not change with models.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

DOCKERFILE="docker/ci/Dockerfile.test"
WORKFLOW=".github/workflows/ci-image.yml"

fail() {
  echo "ci-test-image-models.test.sh: $*" >&2
  exit 1
}

# STT/TTS default bundle dir names come from the export script (subshell, ambient env cleared).
names="$(
  unset SHERPA_STT_MODEL_PATH SHERPA_TTS_MODEL_PATH SHERPA_STT_LANGUAGE SHERPA_STT_BUNDLE SHERPA_TTS_BUNDLE
  export SHERPA_EXPORT_SKIP_VALIDATE=1
  # shellcheck source=/dev/null
  source "$ROOT/scripts/export-sherpa-local-models.sh"
  basename "$SHERPA_STT_MODEL_PATH"
  basename "$SHERPA_TTS_MODEL_PATH"
)"
# Language-ID dir names come from the ensure script's *_DIR="$MODELS_DIR/<name>" assignments.
lang_names="$(sed -n 's/^[A-Z_]*_DIR="\$MODELS_DIR\/\([^"]*\)"$/\1/p' scripts/ci/ensure-sherpa-models.sh)"
[[ "$(printf '%s\n' "$lang_names" | grep -c .)" -eq 3 ]] \
  || fail "expected 3 language-id *_DIR assignments in ensure-sherpa-models.sh, got: $lang_names"
all_names="$(printf '%s\n%s\n' "$names" "$lang_names")"
[[ "$(printf '%s\n' "$all_names" | grep -c .)" -eq 5 ]] || fail "expected 5 model names, got: $all_names"

check_names() {
  local file="$1" name
  while IFS= read -r name; do
    [[ -n "$name" ]] || continue
    grep -qF "/opt/sherpa-models/$name" "$file" \
      || fail "Dockerfile.test does not bake/check $name (ci.image-asset-lockstep)"
  done <<<"$all_names"
}

check_names "$DOCKERFILE"

grep -Eq 'apt-get install[^&]*\bcoturn\b' "$DOCKERFILE" || fail "Dockerfile.test does not install coturn"
grep -qx 'ENV NWR_CI_SHERPA_MODELS=/opt/sherpa-models' "$DOCKERFILE" \
  || fail "Dockerfile.test must set ENV NWR_CI_SHERPA_MODELS=/opt/sherpa-models"

if grep -Eqi 'sherpa-models|coturn' docker/ci/Dockerfile; then
  fail "docker/ci/Dockerfile must not mention sherpa-models or coturn (ci-build fingerprint stays free of test assets)"
fi

# Every COPY source exists and is a push/pull_request path in ci-image.yml.
copy_sources="$(sed -e ':a' -e '/\\$/N; s/\\\n//; ta' "$DOCKERFILE" | awk '/^COPY /{for (i=2;i<NF;i++) print $i}')"
[[ -n "$copy_sources" ]] || fail "no COPY sources found in Dockerfile.test"
while IFS= read -r src; do
  [[ -f "$src" ]] || fail "Dockerfile.test COPY source missing: $src"
  [[ "$(grep -cF "      - '$src'" "$WORKFLOW")" -ge 2 ]] \
    || fail "$src is not in both push and pull_request paths of $WORKFLOW"
done <<<"$copy_sources"

# Self-test: the name check must bite when a model is removed.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
grep -v 'vits-piper-es-glados-medium' "$DOCKERFILE" >"$tmp/Dockerfile.test"
if (check_names "$tmp/Dockerfile.test") >/dev/null 2>&1; then
  fail "self-test: name check did not fail with vits-piper-es-glados-medium removed"
fi

echo "ci-test-image-models.test.sh: OK"
