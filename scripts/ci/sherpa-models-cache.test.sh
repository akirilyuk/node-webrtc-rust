#!/usr/bin/env bash
# Sherpa model cache key + integrity validation contract.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

chmod +x scripts/ci/sherpa-models-cache-key.sh \
  scripts/ci/validate-sherpa-model-dirs.sh \
  scripts/ci/ensure-sherpa-models.sh

KEY_SCRIPT=scripts/ci/sherpa-models-cache-key.sh
k1="$(bash "$KEY_SCRIPT")"
k2="$(bash "$KEY_SCRIPT")"
if [[ "$k1" != "$k2" || ${#k1} -ne 64 ]]; then
  echo "FAIL: unstable sherpa models key" >&2
  exit 1
fi
echo "ok: stable sherpa models key"

action=".github/actions/ci-cache-sherpa-models/action.yml"
if ! grep -q 'sherpa-models-v1-' "$action"; then
  echo "FAIL: action missing sherpa-models-v1 key" >&2
  exit 1
fi
if ! grep -q 'actions/cache/restore@v4' "$action"; then
  echo "FAIL: action must restore without saving before Docker" >&2
  exit 1
fi
if ! grep -q 'validate-sherpa-model-dirs.sh' "$action" \
  || ! grep -q 'cache-valid' "$action"; then
  echo "FAIL: action must validate restored model directories" >&2
  exit 1
fi
if grep -qE 'ensure-sherpa-models\.sh|npm run download-' "$action"; then
  echo "FAIL: host cache restore must not download models without Node/npm" >&2
  exit 1
fi
# Must not share paths with Cargo/native caches
if grep -qiE 'packages/bindings|^\s*target\b|Cargo\.lock' "$action"; then
  echo "FAIL: Sherpa model cache must stay separate from native/Cargo" >&2
  exit 1
fi
echo "ok: action key + restore-only validation + separation"

# Validation fails on missing dirs (USE_ENV keeps injected paths)
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
if SHERPA_VALIDATE_USE_ENV=1 \
  SHERPA_STT_MODEL_PATH="$tmpdir/missing-stt" \
  SHERPA_TTS_MODEL_PATH="$tmpdir/missing-tts" \
  bash scripts/ci/validate-sherpa-model-dirs.sh 2>/dev/null; then
  echo "FAIL: validate should fail for missing dirs" >&2
  exit 1
fi
echo "ok: validate rejects missing dirs"

# Validation fails on empty/corrupt files
mkdir -p "$tmpdir/stt" "$tmpdir/tts"
touch "$tmpdir/stt/encoder.onnx" "$tmpdir/stt/decoder.onnx" "$tmpdir/stt/joiner.onnx" "$tmpdir/stt/tokens.txt"
touch "$tmpdir/tts/voice.onnx" "$tmpdir/tts/tokens.txt"
if SHERPA_VALIDATE_USE_ENV=1 \
  SHERPA_STT_MODEL_PATH="$tmpdir/stt" \
  SHERPA_TTS_MODEL_PATH="$tmpdir/tts" \
  bash scripts/ci/validate-sherpa-model-dirs.sh 2>/dev/null; then
  echo "FAIL: validate should reject empty model files" >&2
  exit 1
fi
echo "ok: validate rejects empty files"

# Ambient polluted paths must not win over catalog defaults
if SHERPA_STT_MODEL_PATH="$tmpdir/missing-stt" \
  SHERPA_TTS_MODEL_PATH="$tmpdir/missing-tts" \
  bash scripts/ci/validate-sherpa-model-dirs.sh >/dev/null 2>&1; then
  echo "ok: ambient bogus paths ignored when real models present (or validate failed closed)"
else
  # OK either way if models missing on this machine; require ignore behavior via path print
  out="$(SHERPA_STT_MODEL_PATH="$tmpdir/missing-stt" bash scripts/ci/validate-sherpa-model-dirs.sh 2>&1 || true)"
  if echo "$out" | rg -q 'missing-stt'; then
    echo "FAIL: validate still used ambient polluted STT path" >&2
    exit 1
  fi
  echo "ok: ambient bogus paths ignored"
fi

# reusable-test restores before Docker and saves only after integration succeeds
if ! grep -q 'ci-cache-sherpa-models' .github/workflows/reusable-test.yml; then
  echo "FAIL: reusable-test must restore Sherpa models before Docker" >&2
  exit 1
fi
if ! grep -q 'Save repaired/downloaded Sherpa model dirs' .github/workflows/reusable-test.yml \
  || ! grep -q 'steps.integration.outcome == .success.' .github/workflows/reusable-test.yml; then
  echo "FAIL: reusable-test must save models after successful Docker integration" >&2
  exit 1
fi
echo "ok: reusable-test restores before Docker and saves after success"

echo "sherpa-models-cache.test.sh: all checks passed"
