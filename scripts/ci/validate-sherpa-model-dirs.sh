#!/usr/bin/env bash
# Validate English Sherpa model directories used by CI E2E (not just existence).
# Exit 0 when usable; non-zero when missing/corrupt (caller should redownload).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# Ignore ambient SHERPA_* overrides (stale/polluted env breaks CI validation).
# Tests may set SHERPA_VALIDATE_USE_ENV=1 to inject temporary paths.
if [[ "${SHERPA_VALIDATE_USE_ENV:-}" != "1" ]]; then
  unset SHERPA_STT_MODEL_PATH SHERPA_TTS_MODEL_PATH SHERPA_STT_LANGUAGE
fi

# shellcheck source=/dev/null
SHERPA_EXPORT_SKIP_VALIDATE=1 source "$ROOT/scripts/export-sherpa-local-models.sh"

fail() {
  echo "validate-sherpa-model-dirs: $*" >&2
  exit 1
}

[[ -d "${SHERPA_STT_MODEL_PATH:-}" ]] || fail "STT dir missing: ${SHERPA_STT_MODEL_PATH:-}"
[[ -d "${SHERPA_TTS_MODEL_PATH:-}" ]] || fail "TTS dir missing: ${SHERPA_TTS_MODEL_PATH:-}"

# STT transducer (Kroko / zipformer)
for f in encoder.onnx decoder.onnx joiner.onnx tokens.txt; do
  [[ -f "$SHERPA_STT_MODEL_PATH/$f" && -s "$SHERPA_STT_MODEL_PATH/$f" ]] \
    || fail "STT missing/empty: $SHERPA_STT_MODEL_PATH/$f"
done

# Piper TTS: at least one non-empty .onnx + tokens
shopt -s nullglob
tts_onnx=("$SHERPA_TTS_MODEL_PATH"/*.onnx)
[[ ${#tts_onnx[@]} -gt 0 ]] || fail "TTS has no .onnx under $SHERPA_TTS_MODEL_PATH"
for f in "${tts_onnx[@]}"; do
  [[ -s "$f" ]] || fail "TTS empty onnx: $f"
done
[[ -f "$SHERPA_TTS_MODEL_PATH/tokens.txt" && -s "$SHERPA_TTS_MODEL_PATH/tokens.txt" ]] \
  || fail "TTS missing/empty tokens.txt"

echo "validate-sherpa-model-dirs: OK"
echo "  STT=$SHERPA_STT_MODEL_PATH"
echo "  TTS=$SHERPA_TTS_MODEL_PATH"
