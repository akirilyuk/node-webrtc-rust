#!/usr/bin/env bash
# Ensure English Sherpa model dirs are present and valid for CI E2E.
# On validation failure (corrupt/partial cache restore), wipe and redownload.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORKSPACE="${SHERPA_EXAMPLE_WORKSPACE:-@node-webrtc-rust/example-voice-agent-local-sherpa}"
TIMEOUT="${CI_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC:-900}"
CI_STEP="$ROOT/scripts/ci/ci-step.sh"
VALIDATE="$ROOT/scripts/ci/validate-sherpa-model-dirs.sh"

# Canonical default paths (do not trust ambient SHERPA_* for rm/redownload).
unset SHERPA_STT_MODEL_PATH SHERPA_TTS_MODEL_PATH SHERPA_STT_LANGUAGE
# shellcheck source=/dev/null
SHERPA_EXPORT_SKIP_VALIDATE=1 source "$ROOT/scripts/export-sherpa-local-models.sh"
STT_DIR="$SHERPA_STT_MODEL_PATH"
TTS_DIR="$SHERPA_TTS_MODEL_PATH"

fail() {
  echo "ensure-sherpa-models: $*" >&2
  exit 1
}

# ci-test image bakes the model dirs under $NWR_CI_SHERPA_MODELS (docker/ci/Dockerfile.test).
# Link them into the example's .models/ so every consumer finds them at the usual paths.
link_baked_models() {
  local root="${NWR_CI_SHERPA_MODELS:-}"
  [[ -n "$root" && -d "$root" ]] || return 0
  local models_dir="$ROOT/examples/voice-agent-local-sherpa/.models" name
  mkdir -p "$models_dir"
  for name in "$(basename "$STT_DIR")" "$(basename "$TTS_DIR")" \
    sherpa-onnx-whisper-tiny vits-piper-de_DE-thorsten-medium vits-piper-es-glados-medium; do
    if [[ ! -e "$models_dir/$name" && -d "$root/$name" ]]; then
      ln -s "$root/$name" "$models_dir/$name"
      echo "==> Sherpa model from image: $name"
    fi
  done
}
link_baked_models

if bash "$VALIDATE"; then
  echo "==> Sherpa models OK (cache or prior download)"
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    echo "redownloaded=false" >>"$GITHUB_OUTPUT"
  fi
else
  echo "==> Sherpa models invalid/missing — clearing and redownloading"
  rm -rf "$STT_DIR" "$TTS_DIR"

  bash "$CI_STEP" --timeout "$TIMEOUT" \
    "sherpa download-stt" -- npm run download-stt:en --workspace="$WORKSPACE"
  bash "$CI_STEP" --timeout "$TIMEOUT" \
    "sherpa download-tts" -- npm run download-tts:en --workspace="$WORKSPACE"

  bash "$VALIDATE"
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    echo "redownloaded=true" >>"$GITHUB_OUTPUT"
  fi
fi

# Language-ID roundtrip needs Whisper tiny LID + de/es Piper TTS (en STT/TTS alone is not enough).
EXAMPLE_DIR="$ROOT/examples/voice-agent-local-sherpa"
MODELS_DIR="$EXAMPLE_DIR/.models"
LID_DIR="$MODELS_DIR/sherpa-onnx-whisper-tiny"
DE_TTS_DIR="$MODELS_DIR/vits-piper-de_DE-thorsten-medium"
ES_TTS_DIR="$MODELS_DIR/vits-piper-es-glados-medium"

ensure_language_id_models() {
  local need=0
  if [[ ! -d "$LID_DIR" ]]; then need=1; fi
  if [[ ! -d "$DE_TTS_DIR" ]]; then need=1; fi
  if [[ ! -d "$ES_TTS_DIR" ]]; then need=1; fi
  if [[ "$need" -eq 0 ]]; then
    echo "==> Sherpa language-id models OK"
    return 0
  fi
  echo "==> Sherpa language-id models missing — downloading LID + de/es TTS"
  bash "$CI_STEP" --timeout "$TIMEOUT" \
    "sherpa download-lid" -- npm run download-lid --workspace="$WORKSPACE"
  bash "$CI_STEP" --timeout "$TIMEOUT" \
    "sherpa download-tts de" -- npm run download-tts --workspace="$WORKSPACE" -- --lang=de
  bash "$CI_STEP" --timeout "$TIMEOUT" \
    "sherpa download-tts es" -- npm run download-tts --workspace="$WORKSPACE" -- --lang=es
  [[ -d "$LID_DIR" ]] || fail "LID dir missing after download: $LID_DIR"
  [[ -d "$DE_TTS_DIR" ]] || fail "German TTS dir missing after download: $DE_TTS_DIR"
  [[ -d "$ES_TTS_DIR" ]] || fail "Spanish TTS dir missing after download: $ES_TTS_DIR"
}

ensure_language_id_models
