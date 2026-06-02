#!/usr/bin/env bash
# Export default local Sherpa model paths for voice-agent-local-sherpa examples.
#
# Defaults match examples/shared/sherpa-local-model-catalog.json (English Kroko STT +
# Piper amy-low TTS) — same bundles as scripts/ci/run-sherpa-example-ci.sh.
#
# Usage:
#   source scripts/export-sherpa-local-models.sh
#   bash scripts/export-sherpa-local-models.sh -- tsx src/index.ts
#   bash scripts/export-sherpa-local-models.sh   # print export lines for eval
#
# Override bundles (optional):
#   SHERPA_STT_BUNDLE=sherpa-onnx-streaming-zipformer-de-kroko-2025-08-06 \
#   SHERPA_STT_LANGUAGE=de source scripts/export-sherpa-local-models.sh
#
# Skip missing-dir check (e.g. before download):
#   SHERPA_EXPORT_SKIP_VALIDATE=1 source scripts/export-sherpa-local-models.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLE_ROOT="$ROOT/examples/voice-agent-local-sherpa"

STT_BUNDLE="${SHERPA_STT_BUNDLE:-sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06}"
TTS_BUNDLE="${SHERPA_TTS_BUNDLE:-vits-piper-en_US-amy-low}"

export SHERPA_STT_MODEL_PATH="${SHERPA_STT_MODEL_PATH:-$EXAMPLE_ROOT/.models/$STT_BUNDLE}"
export SHERPA_TTS_MODEL_PATH="${SHERPA_TTS_MODEL_PATH:-$EXAMPLE_ROOT/.models/$TTS_BUNDLE}"
export SHERPA_STT_LANGUAGE="${SHERPA_STT_LANGUAGE:-en}"

_validate_sherpa_model_dirs() {
  if [[ "${SHERPA_EXPORT_SKIP_VALIDATE:-}" == "1" ]]; then
    return 0
  fi
  local missing=0
  if [[ ! -d "$SHERPA_STT_MODEL_PATH" ]]; then
    echo "Sherpa STT model directory missing: $SHERPA_STT_MODEL_PATH" >&2
    echo "Run: npm run download-stt:en --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa" >&2
    missing=1
  fi
  if [[ ! -d "$SHERPA_TTS_MODEL_PATH" ]]; then
    echo "Sherpa TTS model directory missing: $SHERPA_TTS_MODEL_PATH" >&2
    echo "Run: npm run download-tts:en --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa" >&2
    missing=1
  fi
  if [[ "$missing" -ne 0 ]]; then
    return 1
  fi
}

_print_exports() {
  printf 'export SHERPA_STT_MODEL_PATH=%q\n' "$SHERPA_STT_MODEL_PATH"
  printf 'export SHERPA_TTS_MODEL_PATH=%q\n' "$SHERPA_TTS_MODEL_PATH"
  printf 'export SHERPA_STT_LANGUAGE=%q\n' "$SHERPA_STT_LANGUAGE"
}

# Sourced — export into the caller's shell.
if [[ "${BASH_SOURCE[0]}" != "${0}" ]]; then
  _validate_sherpa_model_dirs
  return 0
fi

_validate_sherpa_model_dirs

if [[ "${1:-}" == "--" ]]; then
  shift
  exec "$@"
fi

_print_exports
